// main.rs

extern crate alloc;

use anyhow::{bail, Context, Result};
use log::{error, info, warn};

use esp_idf_sys as sys; // C-API (MQTT & HTTP)

use alloc::ffi::CString;
use alloc::string::String;
use alloc::string::ToString;

use esp_idf_svc::{
    eventloop::EspSystemEventLoop,
    log::EspLogger,
    nvs::EspDefaultNvsPartition,
    wifi::{AuthMethod, BlockingWifi, ClientConfiguration as StaCfg, Configuration as
WifiCfg, EspWifi},
};

use esp_idf_svc::hal::{
    delay::FreeRtos,
    gpio::{AnyIOPin, Output, PinDriver},
    ledc::{config::TimerConfig, LedcDriver, LedcTimerDriver, Resolution},
    peripherals::Peripherals,
    uart::{config::Config as UartConfig, UartDriver},
    units::Hertz,
};

use serde_json::json;

// ===== tambahan untuk TCP server & concurrency =====
use std::io::Write as IoWrite;
use std::net::{TcpListener, TcpStream};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

// = = = = = = = = = = = = = = = = = = = KONFIGURASI = = = = = = = = = = = = = = = = = = =
// Wi-Fi
const WIFI_SSID: &str = "mncplay5G-1";
const WIFI_PASS: &str = "12345678";

// ThingsBoard Cloud (MQTT Basic)
const TB_MQTT_URL: &str = "mqtt://mqtt.thingsboard.cloud:1883";
const TB_CLIENT_ID: &str = "esp32s3-kelompok5";
const TB_USERNAME: &str = "86ad8020-adc7-11f0-9dac-f14aa7f7559f";
const TB_PASSWORD: &str = "";

// InfluxDB (Cloud atau lokal)
// Contoh Cloud: "https://us-east-1-1.aws.cloud2.influxdata.com"
// Contoh Lokal : "http://localhost:8086"
const INFLUX_URL: &str = "https://us-east-1-1.aws.cloud2.influxdata.com";
const INFLUX_ORG_ID: &str = "91620fda2236551a"; // atau UUID org Cloud kamu
const INFLUX_BUCKET: &str = "sktkelompok5";
const INFLUX_TOKEN: &str = "PuLngOdbwVI2iM5X8wCo0LgZv6mgsTSK2GIeKdAwBnPxU9NxJVbPs93h3rxqBH4KVrrDqKMADaub1FwNh_levQ==";

// Modbus (SHT20 via RS485)
const MODBUS_ID: u8 = 0x01;
const BAUD: u32 = 9_600;

// TCP Server (untuk stream JSON ke klien)
const TCP_LISTEN_ADDR: &str = "0.0.0.0";
const TCP_LISTEN_PORT: u16 = 7878;

// Relay (AKTIF saat RH > 60%)
const RELAY_GPIO_IS_LOW_ACTIVE: bool = true; // true: aktif-LOW (umum); false: aktif-
HIGH
const RH_ON_THRESHOLD: f32 = 59.0;
// >60% RH menyalakan relay

// = = = = = = = = = = = = = = = = = = = Util = = = = = = = = = = = = = = = = = = =
#[inline(always)]
fn ms_to_ticks(ms: u32) -> u32 {
    (ms as u64 * sys::configTICK_RATE_HZ as u64 / 1000) as u32
}

fn looks_like_uuid(s: &str) -> bool {
    s.len() == 36 && s.matches('-').count() == 4
}

// Minimal percent-encoding untuk komponen query (RFC 3986 unreserved: ALNUM -_.~)
fn url_encode_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.as_bytes() {
        let c = *b as char;
        if c.is_ascii_alphanumeric() || "-_.~".contains(c) {
            out.push(c);
        } else {
            let _ = core::fmt::write(&mut out, format_args!("%{:02X}", b));
        }
    }
    out
}

// = = = = = = = = = = = = = = = = = = = MQTT client (C-API) = = = = = = = = = = = = = = = = = = =
struct SimpleMqttClient {
    client: *mut sys::esp_mqtt_client,
}

impl SimpleMqttClient {
    fn new(broker_url: &str, username: &str, password: &str, client_id: &str) ->
Result<Self> {
        unsafe {
            let broker_url_cstr = CString::new(broker_url)?;
            let username_cstr = CString::new(username)?;
            let password_cstr = CString::new(password)?;
            let client_id_cstr = CString::new(client_id)?;

            let mut cfg: sys::esp_mqtt_client_config_t = core::mem::zeroed();
            cfg.broker.address.uri = broker_url_cstr.as_ptr() as *const u8;
            cfg.credentials.username = username_cstr.as_ptr() as *const u8;
            cfg.credentials.client_id = client_id_cstr.as_ptr() as *const u8;
            cfg.credentials.authentication.password = password_cstr.as_ptr() as *const
u8;
            cfg.session.keepalive = 30; // detik
            cfg.network.timeout_ms = 20_000; // ms


            let client = sys::esp_mqtt_client_init(&cfg);
            if client.is_null() {
                bail!("Failed to initialize MQTT client");
            }
            let err = sys::esp_mqtt_client_start(client);
            if err != sys::ESP_OK {
                bail!("Failed to start MQTT client, esp_err=0x{:X}", err as u32);
            }

            sys::vTaskDelay(ms_to_ticks(2500));
            Ok(Self { client })

        }
    }

    fn publish(&self, topic: &str, data: &str) -> Result<()> {
        unsafe {
            let topic_c = CString::new(topic)?;
            let msg_id = sys::esp_mqtt_client_publish(
                self.client,
                topic_c.as_ptr(),
                data.as_ptr(),
                data.len() as i32,
                1,
                0,
            );
            if msg_id < 0 {
                bail!("Failed to publish message, code: {}", msg_id);
            }
            info!("MQTT published (id={})", msg_id);
            Ok(())
        }
    }

}


impl Drop for SimpleMqttClient {
    fn drop(&mut self) {
        unsafe {
            sys::esp_mqtt_client_stop(self.client);
            sys::esp_mqtt_client_destroy(self.client);
        }
    }
}

// = = = = = = = = = = = = = = = = = = = CRC & Modbus util = = = = = = = = = = = = = = = = = = =
fn crc16_modbus(mut crc: u16, byte: u8) -> u16 {
    crc ^= byte as u16;
    for _ in 0..8 {
        crc = if (crc & 1) != 0 { (crc >> 1) ^ 0xA001 } else { crc >> 1 };
    }
    crc
}
fn modbus_crc(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data { crc = crc16_modbus(crc, b); }
    crc
}
fn build_read_req(slave: u8, func: u8, start_reg: u16, qty: u16) -> heapless::Vec<u8,
256> {
    use heapless::Vec;
    let mut pdu: Vec<u8, 256> = Vec::new();
    pdu.push(slave).unwrap();
    pdu.push(func).unwrap();
    pdu.push((start_reg >> 8) as u8).unwrap();
    pdu.push((start_reg & 0xFF) as u8).unwrap();
    pdu.push((qty >> 8) as u8).unwrap();
    pdu.push((qty & 0xFF) as u8).unwrap();
    let crc = modbus_crc(&pdu);
    pdu.push((crc & 0xFF) as u8).unwrap();
    pdu.push((crc >> 8) as u8).unwrap();
    pdu
}
fn parse_read_resp(expected_slave: u8, qty: u16, buf: &[u8]) -> Result<heapless::Vec<
u16, 64>> {
    use heapless::Vec;
    if buf.len() >= 5 && (buf[1] & 0x80) != 0 {
        let crc_rx = u16::from(buf[4]) << 8 | u16::from(buf[3]);
        let crc_calc = modbus_crc(&buf[..3]);
        if crc_rx == crc_calc {
            let code = buf[2];
            bail!("Modbus exception 0x{:02X}", code);
        } else {
            bail!("Exception frame CRC mismatch");
        }
    }
    let need = 1 + 1 + 1 + (2 * qty as usize) + 2;
    if buf.len() < need { bail!("Response too short: got {}, need {}", buf.len(), need
); }
    if buf[0] != expected_slave { bail!("Unexpected slave id: got {}, expected {}",
buf[0], expected_slave); }
    if buf[1] != 0x03 && buf[1] != 0x04 { bail!("Unexpected function code: 0x{:02X}",
buf[1]); }
    let bc = buf[2] as usize;
    if bc != 2 * qty as usize { bail!("Unexpected byte count: {}", bc); }
    let crc_rx = u16::from(buf[need - 1]) << 8 | u16::from(buf[need - 2]);
    let crc_calc = modbus_crc(&buf[..need - 2]);
    if crc_rx != crc_calc { bail!("CRC mismatch: rx=0x{:04X}, calc=0x{:04X}", crc_rx,
crc_calc); }

    let mut out: Vec<u16, 64> = Vec::new();
    for i in 0..qty as usize {
        let hi = buf[3 + 2 * i] as u16;
        let lo = buf[3 + 2 * i + 1] as u16;
        out.push((hi << 8) | lo).unwrap();
    }
    Ok(out)
}

// = = = = = = = = = = = = = = = = = = = RS485 helpers = = = = = = = = = = = = = = = = = = =
fn rs485_write(
    uart: &UartDriver<'_>,
    de: &mut PinDriver<'_, esp_idf_svc::hal::gpio::Gpio21, Output>,
    data: &[u8],
) -> Result<()> {
    de.set_high()?;
    FreeRtos::delay_ms(3);
    uart.write(data)?;
    uart.wait_tx_done(200)?;
    de.set_low()?;
    FreeRtos::delay_ms(3);
    Ok(())
}
fn rs485_read(uart: &UartDriver<'_>, dst: &mut [u8], ticks: u32) -> Result<usize> {
    uart.clear_rx()?;
    let n = uart.read(dst, ticks)?;
    use core::fmt::Write as _;
    let mut s = String::new();
    for b in &dst[..n] { write!(&mut s, "{:02X} ", b).ok(); }
    info!("RS485 RX {} bytes: {}", n, s);
    Ok(n)
}
fn try_read(
    uart: &UartDriver<'_>,
    de: &mut PinDriver<'_, esp_idf_svc::hal::gpio::Gpio21, Output>,
    func: u8, start: u16, qty: u16, ticks: u32,
) -> Result<heapless::Vec<u16, 64>> {
    let req = build_read_req(MODBUS_ID, func, start, qty);
    rs485_write(uart, de, &req)?;
    let mut buf = [0u8; 64];
    let n = rs485_read(uart, &mut buf, ticks)?;
    parse_read_resp(MODBUS_ID, qty, &buf[..n])
}
fn probe_map(
    uart: &UartDriver<'_>,
    de: &mut PinDriver<'_, esp_idf_svc::hal::gpio::Gpio21, Output>,
) -> Option<(u8, u16, u16)> {
    for &fc in &[0x04u8, 0x03u8] {
        for start in 0x0000u16..=0x0010u16 {
            for &qty in &[1u16, 2u16] {
                if let Ok(regs) = try_read(uart, de, fc, start, qty, 250) {
                    info!("FOUND: fc=0x{:02X}, start=0x{:04X}, qty={}, regs={:04X?}",
fc, start, qty, regs.as_slice());
                    return Some((fc, start, qty));
                }
            }
        }
    }
    None
}
fn read_sht20_with_map(
    uart: &UartDriver<'_>,
    de: &mut PinDriver<'_, esp_idf_svc::hal::gpio::Gpio21, Output>,
    fc: u8, start: u16, qty: u16,
) -> Result<(f32, f32)> {
    let regs = try_read(uart, de, fc, start, qty, 250)?;
    let (raw_t, raw_h) = if regs.len() >= 2 { (regs[0], regs[1]) } else { (regs[0], 0)
};
    let temp_c = (raw_t as f32) * 0.1;
    let rh_pct = (raw_h as f32) * 0.1;
    Ok((temp_c, rh_pct))
}

// = = = = = = = = = = = = = = = = = = = Wi-Fi (BlockingWifi) = = = = = = = = = = = = = = = = = = =
fn connect_wifi(wifi: &mut BlockingWifi<EspWifi<'static>>) -> Result<()> {
    let cfg = WifiCfg::Client(StaCfg {
        ssid: heapless::String::try_from(WIFI_SSID).unwrap(),
        password: heapless::String::try_from(WIFI_PASS).unwrap(),
        auth_method: AuthMethod::WPA2Personal,
        channel: None,
        ..Default::default()
    });
    wifi.set_configuration(&cfg)?;
    wifi.start()?;
    info!("Wi-Fi driver started");
    wifi.connect()?;
    info!("Wi-Fi connect issued, waiting for netif up...");
    wifi.wait_netif_up()?;
    let ip = wifi.wifi().sta_netif().get_ip_info()?;
    info!("Wi-Fi connected. IP = {}", ip.ip);
    unsafe { sys::vTaskDelay(ms_to_ticks(1200)); }
    Ok(())
}

// = = = = = = = = = = = = = = = = = = = Influx helpers = = = = = = = = = = = = = = = = = = =
fn influx_line(measurement: &str, device: &str, t_c: f32, h_pct: f32) -> String {
    format!("{},device={} temperature_c={},humidity_pct={}", measurement, device, t_c,
h_pct)
}


fn influx_write(lp: &str) -> Result<()> {
    unsafe {
        let org_q = if looks_like_uuid(INFLUX_ORG_ID) { "orgID" } else { "org" };
        let url = format!(
            "{}/api/v2/write?{}={}&bucket={}&precision=ms",
            INFLUX_URL,
            org_q,
            url_encode_component(INFLUX_ORG_ID),
            url_encode_component(INFLUX_BUCKET)
        );
        let url_c = CString::new(url.as_str())?;

        let mut cfg: sys::esp_http_client_config_t = core::mem::zeroed();
        cfg.url = url_c.as_ptr();
        cfg.method = sys::esp_http_client_method_t_HTTP_METHOD_POST;


        if INFLUX_URL.starts_with("https://") {
            cfg.transport_type = sys::
esp_http_client_transport_t_HTTP_TRANSPORT_OVER_SSL;
            cfg.crt_bundle_attach = Some(sys::esp_crt_bundle_attach);
        }

        let client = sys::esp_http_client_init(&cfg);
        if client.is_null() {
            bail!("esp_http_client_init failed");
        }

        // headers
        let h_auth = CString::new("Authorization")?;
        let v_auth = CString::new(format!("Token {}", INFLUX_TOKEN))?;
        let h_ct = CString::new("Content-Type")?;
        let v_ct = CString::new("text/plain; charset=utf-8")?;
        let h_acc = CString::new("Accept")?;
        let v_acc = CString::new("application/json")?;
        let h_conn = CString::new("Connection")?;
        let v_conn = CString::new("close")?;


        sys::esp_http_client_set_header(client,
        sys::esp_http_client_set_header(client,
        sys::esp_http_client_set_header(client,
        sys::esp_http_client_set_header(client,
                                        h_auth.as_ptr(), v_auth.as_ptr());
                                        h_ct.as_ptr(), v_ct.as_ptr());
                                        h_acc.as_ptr(), v_acc.as_ptr());
                                        h_conn.as_ptr(), v_conn.as_ptr());

        // body (Line Protocol)
        sys::esp_http_client_set_post_field(client, lp.as_ptr(), lp.len() as i32);


        // perform
        let err = sys::esp_http_client_perform(client);
        if err != sys::ESP_OK {
            let e = format!("esp_http_client_perform failed: 0x{:X}", err as u32);
            sys::esp_http_client_cleanup(client);
            bail!(e);
        }

        let status = sys::esp_http_client_get_status_code(client);
        if status != 204 {
            let mut body_buf = [0u8; 256];
            let read = sys::esp_http_client_read_response(client, body_buf.as_mut_ptr
(), body_buf.len() as i32);
            let body = if read > 0 {
                core::str::from_utf8(&body_buf[..read as usize]).unwrap_or("")
            } else { "" };
            warn!("Influx write failed: HTTP {} Body: {} ", status, body);
            sys::esp_http_client_cleanup(client);
            bail!("Influx write HTTP status {}", status);
        } else {
            info!("OK Data Berhasil Dikirim ke InfluxDB");
        }
        sys::esp_http_client_cleanup(client);
        Ok(())

    }

}

// = = = = = = = = = = = = = = = = = = = Servo helpers (LEDC) = = = = = = = = = = = = = = = = = = =
struct Servo {
    ch: LedcDriver<'static>,
    duty_0: u32,
    duty_90: u32,
    duty_180: u32,
}

impl Servo {
    fn new(mut ch: LedcDriver<'static>) -> Result<Self> {
        let max = ch.get_max_duty() as u64; // Bits14 -> 16383
        let period_us = 20_000u64;
        // 50 Hz -> 20 ms
        let duty_from_us = |us: u32| -> u32 { ((max * us as u64) / period_us) as u32
};

        let duty_0 = duty_from_us(500);
        // ~0.5 ms (=0 deg)
        let duty_90 = duty_from_us(1500); // ~1.5 ms (=90 deg)
        let duty_180 = duty_from_us(2500); // ~2.5 ms (=180 deg)


        // Posisi awal: 90 deg
        ch.set_duty(duty_90)?;
        ch.enable()?;

        Ok(Self { ch, duty_0, duty_90, duty_180 })

    }

    fn set_0(&mut self) -> Result<()> { self.ch.set_duty(self.duty_0).map_err(Into::
into) }
    fn set_90(&mut self) -> Result<()> { self.ch.set_duty(self.duty_90).map_err(Into::
into) }
    fn set_180(&mut self) -> Result<()> { self.ch.set_duty(self.duty_180).map_err(Into
::into) }

}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum ServoPos { P0, P90, P180 }

// = = = = = = = = = = = = = = = = = = = TCP server helpers = = = = = = = = = = = = = = = = = = =

fn start_tcp_server() -> mpsc::Sender<String> {
    let (tx, rx) = mpsc::channel::<String>();
    let clients: Arc<Mutex<Vec<TcpStream>>> = Arc::new(Mutex::new(Vec::new()));

    // Thread acceptor
    {
        let clients_accept = Arc::clone(&clients);
        thread::spawn(move || {
            let addr = format!("{}:{}", TCP_LISTEN_ADDR, TCP_LISTEN_PORT);
            loop {
                match TcpListener::bind(&addr) {
                    Ok(listener) => {
                        info!("TCP Server listening on {}", addr);
                        listener.set_nonblocking(true).ok(); // non-blocking accept
                        loop {
                            match listener.accept() {
                                Ok((stream, peer)) => {
                                    let _ = stream.set_nodelay(true);
                                    info!("TCP client connected: {}", peer);
                                    if let Ok(mut vec) = clients_accept.lock() {
                                        vec.push(stream);
                                    }
                                }
                                Err(ref e) if e.kind() == std::io::ErrorKind::
WouldBlock => {
                                    // tidak ada koneksi baru saat ini
                                    FreeRtos::delay_ms(100);
                                }
                                Err(e) => {
                                    warn!("TCP accept error: {} (rebind)", e);
                                    FreeRtos::delay_ms(1000);
                                    break; // keluar loop dalam -> rebind listener
                                }
                            }
                            // kecilkan beban CPU
                            FreeRtos::delay_ms(10);
                        }
                    }
                    Err(e) => {
                        warn!("TCP bind {} error: {} (retry in 1s)", addr, e);
                        FreeRtos::delay_ms(1000);
                    }
                }
            }
        });
    }

    // Thread broadcaster (writer)
    {
        let clients_write = Arc::clone(&clients);
        thread::spawn(move || {
            while let Ok(line) = rx.recv() {
                if let Ok(mut vec) = clients_write.lock() {
                    vec.retain_mut(|stream| {
                        if writeln!(stream, "{}", line).is_err() {
                            warn!("TCP write to client failed: drop client");
                            false
                        } else {
                            true
                        }
                    });
                }
            }
        });
    }

    tx
}

// ===== Relay helper =====
#[inline(always)]
fn set_relay(
    relay: &mut PinDriver<'_, esp_idf_svc::hal::gpio::Gpio5, Output>,
    on: bool,
    active_low: bool,
) -> anyhow::Result<()> {
    if active_low {
        if on { relay.set_low()?; } else { relay.set_high()?; }
    } else {
        if on { relay.set_high()?; } else { relay.set_low()?; }
    }
    Ok(())
}

// Helper: satu siklus baca + publish + tulis Influx + kirim ke TCP server
fn do_sensor_io(
    uart: &UartDriver<'_>,
    de_pin: &mut PinDriver<'_, esp_idf_svc::hal::gpio::Gpio21, Output>,
    fc_use: u8, start_use: u16, qty_use: u16,
    mqtt: &SimpleMqttClient,
    topic_tele: &str,
    tcp_tx: &mpsc::Sender<String>,
) -> Result<(f32, f32)> {
    let (t, h) = read_sht20_with_map(uart, de_pin, fc_use, start_use, qty_use)?;
    let ts_ms = unsafe { sys::esp_timer_get_time() } / 1000;
    let t_rounded = (t * 10.0).round() / 10.0;
    let h_rounded = (h * 10.0).round() / 10.0;

    let payload = json!({
        "sensor": "sht20",
        "temperature_c": t_rounded,
        "humidity_pct": h_rounded,
        "ts_ms": ts_ms
    }).to_string();

    // 1) log ke stdout
    println!("{}", payload);

    // 2) publish ke ThingsBoard via MQTT
    if let Err(e) = mqtt.publish(topic_tele, &payload) {
        error!("MQTT publish error: {e:?}");
    }

    // 3) tulis ke Influx (HTTP)
    let lp = influx_line("sht20", TB_CLIENT_ID, t_rounded, h_rounded);
    if let Err(e) = influx_write(&lp) {
        warn!("Influx write failed: {e}");
    }

    // 4) kirim ke semua klien TCP yang terhubung
    if let Err(e) = tcp_tx.send(payload) {
        warn!("TCP channel send failed: {e}");
    }

    Ok((t, h))

}

// = = = = = = = = = = = = = = = = = = = main = = = = = = = = = = = = = = = = = = =
fn main() -> Result<()> {
    // ESP-IDF init
    sys::link_patches();
    EspLogger::initialize_default();
    info!("Modbus RS485 + ThingsBoard MQTT + InfluxDB + Servo + TCP Server + Relay");

    // Peripherals & services
    let peripherals = Peripherals::take().context("Peripherals::take")?;
    let pins = peripherals.pins;
    let sys_loop = EspSystemEventLoop::take().context("eventloop")?;
    let nvs = EspDefaultNvsPartition::take().context("nvs")?;

    // Wi-Fi via BlockingWifi
    let mut wifi = BlockingWifi::wrap(
        EspWifi::new(peripherals.modem, sys_loop.clone(), Some(nvs))?,
        sys_loop,
    )?;
    connect_wifi(&mut wifi)?;

    // MQTT ThingsBoard (Basic)
    let mqtt = SimpleMqttClient::new(TB_MQTT_URL, TB_USERNAME, TB_PASSWORD,
TB_CLIENT_ID)?;
    info!("MQTT connected to {}", TB_MQTT_URL);

    // === Start TCP Server (listener) ===
    let tcp_tx = start_tcp_server();
    info!("TCP server spawned at {}:{}", TCP_LISTEN_ADDR, TCP_LISTEN_PORT);

    // UART0 + RS485 (GPIO43/44, DE: GPIO21)
    let tx = pins.gpio43; // U0TXD
    let rx = pins.gpio44; // U0RXD
    let de = pins.gpio21;
    let cfg = UartConfig::new().baudrate(Hertz(BAUD));
    let uart = UartDriver::new(peripherals.uart0, tx, rx, None::<AnyIOPin>, None::<
AnyIOPin>, &cfg)
        .context("UartDriver::new")?;
    let mut de_pin = PinDriver::output(de).context("PinDriver::output(DE)")?;
    de_pin.set_low()?; // default RX
    info!("UART0 ready (TX=GPIO43, RX=GPIO44, DE=GPIO21), {} bps", BAUD);

    // ===== Servo init (LEDC 50 Hz, 14-bit) =====
    let ledc = peripherals.ledc;
    let mut servo_timer = LedcTimerDriver::new(
        ledc.timer0,
        &TimerConfig {
            frequency: Hertz(50),
            resolution: Resolution::Bits14,
            ..Default::default()
        },
    )?;

    let servo_channel = LedcDriver::new(ledc.channel0, &mut servo_timer, pins.gpio18)
?;
    let mut servo = Servo::new(servo_channel)?;
    let mut servo_pos = ServoPos::P90; // posisi awal 90 deg

    // === Relay init (GPIO5) ===
    let mut relay = PinDriver::output(pins.gpio5).context("PinDriver::output(RELAY
GPIO5)")?;
    // Pastikan OFF saat mulai
    if RELAY_GPIO_IS_LOW_ACTIVE { relay.set_high()?; } else { relay.set_low()?; }
    info!("Relay siap di GPIO5 (aktif-{}).", if RELAY_GPIO_IS_LOW_ACTIVE { "LOW" }
else { "HIGH" });

    // Tanda waktu siklus reset servo (ms)
    let mut next_reset_ms: u64 = unsafe { (sys::esp_timer_get_time() as u64) / 1000 }
+ 20_000;

    // Probe mapping registri SHT20 (opsional)
    let (mut fc_use, mut start_use, mut qty_use) = (0x04u8, 0x0000u16, 2u16);
    if let Some((fc, start, qty)) = probe_map(&uart, &mut de_pin) {
        (fc_use, start_use, qty_use) = (fc, start, qty);
        info!("Using map: fc=0x{:02X}, start=0x{:04X}, qty={}", fc_use, start_use,
qty_use);
    } else {
        warn!("Probe failed. Fallback map: fc=0x{:02X}, start=0x{:04X}, qty={}",
fc_use, start_use, qty_use);
    }

    // Loop utama
    let topic_tele = "v1/devices/me/telemetry";
    loop {
        let now_ms: u64 = unsafe { (sys::esp_timer_get_time() as u64) / 1000 };

        if now_ms >= next_reset_ms {
            // === Reset siklus 20 detik -> kembali ke 90 deg ===
            if servo_pos != ServoPos::P90 {
                if let Err(e) = servo.set_90() {
                    error!("Servo reset 90 deg error: {e:?}");
                } else {
                    info!("Reset siklus 20s: Servo -> 90 deg");
                    servo_pos = ServoPos::P90;
                }
            } else {
                info!("Reset siklus 20s: Servo sudah di 90 deg");
            }

            // Baca ulang sensor + kirim data
            match do_sensor_io(&uart, &mut de_pin, fc_use, start_use, qty_use, &mqtt,
topic_tele, &tcp_tx) {
                Ok((t, h)) => {
                    // === Relay logic: ON jika RH > 60%, selain itu OFF ===
                    let want_on = h > RH_ON_THRESHOLD;
                    set_relay(&mut relay, want_on, RELAY_GPIO_IS_LOW_ACTIVE);
                    let lvl = relay.is_set_high();
                    info!("Relay {} (RH={:.1}%) | GPIO5 level={}", if want_on { "ON" }
else { "OFF" }, h, if lvl { "HIGH" } else { "LOW" });

                    // Jeda 5 detik setelah mendapat data
                    FreeRtos::delay_ms(5_000);

                    // Aturan servo (setelah reset)
                    if t < 25.0 {
                        if servo_pos != ServoPos::P180 {
                            if let Err(e) = servo.set_180() { error!("Servo set 180 deg
error: {e:?}"); }
                            else { info!("Servo -> 180 deg (T={:.1} degC) setelah reset"
, t); servo_pos = ServoPos::P180; }
                        }
                    } else if t > 25.0 {
                        if servo_pos != ServoPos::P0 {
                            if let Err(e) = servo.set_0() { error!("Servo set 0 deg
error: {e:?}"); }
                            else { info!("Servo -> 0 deg (T={:.1} degC) setelah reset",
t); servo_pos = ServoPos::P0; }
                        }
                    } else {
                        info!("T=25.0 degC persis -> Servo tetap di {:?}", servo_pos);
                    }

                }
                Err(e) => error!("Modbus read error (after 20s reset): {e:?}"),

            }

            next_reset_ms = now_ms + 20_000;
        } else {
            // === Siklus normal: baca sensor + kirim data ===
            match do_sensor_io(&uart, &mut de_pin, fc_use, start_use, qty_use, &mqtt,
topic_tele, &tcp_tx) {
                Ok((t, h)) => {
                    // === Relay logic: ON jika RH > 60%, selain itu OFF ===
                    let want_on = h > RH_ON_THRESHOLD;
                    set_relay(&mut relay, want_on, RELAY_GPIO_IS_LOW_ACTIVE)?;
                    let lvl = relay.is_set_high();
                    info!("Relay {} (RH={:.1}%) | GPIO5 level={}", if want_on { "ON" }
else { "OFF" }, h, if lvl { "HIGH" } else { "LOW" });

                    // Jeda 5 detik setelah mendapat data
                    FreeRtos::delay_ms(5_000);

                    // Aturan servo (normal)
                    if t < 33.5 {
                        if servo_pos != ServoPos::P180 {
                            if let Err(e) = servo.set_180() { error!("Servo set 180 deg
error: {e:?}"); }
                            else { info!("Servo -> 180 deg (T={:.1} degC)", t);
servo_pos = ServoPos::P180; }
                        }
                    } else if t > 33.5 {
                        if servo_pos != ServoPos::P0 {
                            if let Err(e) = servo.set_0() { error!("Servo set 0 deg
error: {e:?}"); }
                            else { info!("Servo -> 0 deg (T={:.1} degC)", t); servo_pos
= ServoPos::P0; }
                        }
                    } else {
                        info!("T=33.5 degC persis -> Servo tetap di {:?}", servo_pos);
                    }

                }
                Err(e) => error!("Modbus read error: {e:?}"),

            }
        }

        // Delay kecil agar loop tidak terlalu ketat
        FreeRtos::delay_ms(1000);

    }

}
