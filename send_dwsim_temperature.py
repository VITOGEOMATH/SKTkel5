import os
import xml.etree.ElementTree as ET
from datetime import datetime
from influxdb_client import InfluxDBClient, Point, WriteOptions

# === KONFIGURASI ===
XML_PATH = os.path.expanduser("~/bchain/CSTR DWSIM.xml")

# InfluxDB konfigurasi
INFLUX_URL = "http://localhost:8086"
INFLUX_TOKEN = "PuLngOdbwVI2iM5X8wCo0LgZv6mgsTSK2GIeKdAwBnPxU9NxJVbPs93h3rxqBH4KVrrDqKMADaub1FwNh_levQ=="
INFLUX_ORG = "its"
INFLUX_BUCKET = "sktkelompok5"

# === Fungsi untuk membaca temperatur dari XML ===
def read_temperature_from_dwsim(xml_path):
    tree = ET.parse(xml_path)
    root = tree.getroot()

    temps = []
    for elem in root.iter("temperature"):
        try:
            temps.append(float(elem.text))
        except (TypeError, ValueError):
            continue

    if not temps:
        raise ValueError("❌ Tidak ditemukan elemen <temperature> di file XML.")
    
    # Ambil rata-rata (jika lebih dari satu phase)
    avg_temp = sum(temps) / len(temps)
    print(f"📘 Ditemukan {len(temps)} temperatur, rata-rata: {avg_temp:.2f} K")
    return avg_temp

# === Fungsi kirim ke InfluxDB ===
def send_to_influxdb(temp_value):
    with InfluxDBClient(url=INFLUX_URL, token=INFLUX_TOKEN, org=INFLUX_ORG) as client:
        write_api = client.write_api(write_options=WriteOptions(batch_size=1))
        point = (
            Point("cstr_temperature")
            .tag("source", "DWSIM")
            .field("temperature_K", temp_value)
            .field("temperature_C", temp_value - 273.15)
            .time(datetime.utcnow())
        )
        write_api.write(bucket=INFLUX_BUCKET, record=point)
        print(f"✅ Data suhu terkirim ke InfluxDB: {temp_value:.2f} K ({temp_value - 273.15:.2f} °C)")

# === MAIN PROGRAM ===
if __name__ == "__main__":
    try:
        temperature = read_temperature_from_dwsim(XML_PATH)
        send_to_influxdb(temperature)
    except Exception as e:
        print(f"⚠️ Error: {e}")
