<img width="1280" height="701" alt="image" src="https://github.com/user-attachments/assets/478efc21-0a9f-4d35-a63d-bcae2e1cf7a1" />

# “SISTEM KONTROL TERDISTRIBUSI KEL.5 - Sistem Monitoring dan Kontrol Terintegrasi”
Proyek ini dikembangkan untuk memenuhi tugas Sistem Komputasi Terdistribusi (SKT) yang menggabungkan pembacaan sensor SHT20, simulasi DWSIM, database InfluxDB, dan platform IoT ThingsBoard.

 ## Authors
1. Ahmad Radhy                  (Supervisor)
2. Muhammad Rif'al Faiz Arivito (2042231067)
3. Sahal Rajenda                (2042231036)

# Untuk Folder Kelompok dapat diakses di tautan berikut:
## Link Drive:
### [https://its.id/m/LaporanSKTkel13](https://drive.google.com/drive/folders/1-Qk3j-dfDa_lYPk5rcxrHjw7rF0nqEbf)

## Struktur Proyek
```
├── DWsim/
│   ├── dwsim.py          # Python bridge → InfluxDB
│   └── Thingsboard.py    # Python bridge from InfluxDB → Thingsboard
└── Modbus-Edge/
    ├── src/              # Source code utama ESP32-S3
    ├── Cargo.toml        # Konfigurasi & dependensi proyek Rust
    ├── build.rs          # Script build firmware
    ├── target/           # Hasil build firmware
    └── influxdb.py       # Code untuk mengirim data bacaan sensor ke InfluxDB
```

## Requirements
### Hardware
1. ESP32S3
2. Industrial SHT40 Temperature and Humidity Sensor
3. TTL to RS485 MAX485
4. Mini Servo SG90
5. Relay Module 5V High Level Trigger
   
### Software
1. Rust Programming Language V 1.77
2. ESP IDF V 5.2.5
3. esp-idf-svc "=0.51.0"
4. esp-idf-sys = { version = "=0.36.1", features = ["binstart"] }
5. extensa-esp32s3-espidf
6. ldproxy
7. Python 3.13
8. InfluxDB Client
9. Thingsboard
 





