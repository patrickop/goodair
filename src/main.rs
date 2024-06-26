#![feature(generic_const_exprs)]
use std::sync::Arc;
use std::ffi::CString;
use core::sync::atomic::AtomicU64;
use std::time::Duration;
use prometheus_client::encoding::{text::encode, EncodeLabelSet};
use prometheus_client::metrics::family::Family;
use prometheus_client::registry::Registry;
use prometheus_client::metrics::gauge::Gauge;
use tide::{Request, Response, StatusCode};
use async_std::task;
use prometheus_client::registry::Unit::Other;

const CRC8_POLYNOMIAL: u8 = 0x31;
const CRC8_INIT: u8 = 0xFF;
const START_PERIODIC_MEASUREMENT:u16 = 0x21b1;
const READ_MEASUREMENT:u16 = 0xec05;
//const GET_STATUS_READY:u16 = 0xe4b8;
const I2C_SLAVE:u64 = 0x0703;
const STOP_PERIODIC_MEASUREMENT:u16 = 0x3f86;
const SCD40_ADDRESS:u64 = 0x62;
const PMSA003I_ADDRESS:u64 = 0x12;
const BUS:&str = "/dev/i2c-1";

struct I2CDevice {
    fd: std::os::unix::io::RawFd,
}


impl I2CDevice {
    fn new(bus_path: &str, address: &u64) -> std::result::Result<Self, std::io::Error> {
        let c_bus_path = CString::new(bus_path).unwrap();
        let fd = unsafe {libc::open(c_bus_path.as_ptr(), libc::O_RDWR)};
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        if unsafe { libc::ioctl(fd,I2C_SLAVE, *address) }  < 0 {
            return Err(std::io::Error::last_os_error());
        }
        return Ok(I2CDevice { fd })
        
    }
}

impl Drop for I2CDevice {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

struct SCD40Session {
    device: I2CDevice,
}

impl SCD40Session {
    async fn new(device: I2CDevice) -> std::result::Result<Self, String> {
        if !sensiron_send(&device,STOP_PERIODIC_MEASUREMENT) {
            return Err("failed to end session".to_string());
        }
        async_std::task::sleep(Duration::from_millis(1000)).await;
        println!("sending start measurement");
        if !sensiron_send(&device,START_PERIODIC_MEASUREMENT) {
            return Err("failed to start session".to_string());
        }
        async_std::task::sleep(Duration::from_millis(5100)).await;
        println!("SCD40 ready");
        Ok(SCD40Session {device})
    }
    //fn read_if_available(self) -> Option<[i32;3]> {
    //    sensiron_send(&session.device, GET_STATUS_READY);
    //    sleep(Duration::from_millis(1));

    //    sensiron_send(&session.device, read_measurement);
    //    sleep(Duration::from_millis(1));

    //    let [co2, temp_raw, rh_raw] = sensiron_read_3_u16(&session.device).expect("cannot read co2");
    //    let temp = -45.0 + 175.0 * ( temp_raw as f32 / 65536.0);
    //    let rh = 100.0 * ( rh_raw as f32 / 65536.0);
}

impl Drop for SCD40Session {
    fn drop(&mut self) {
        println!("ending measurment");
        sensiron_send(&self.device,STOP_PERIODIC_MEASUREMENT);
    }
}



fn sensirion_common_generate_crc(data: &[u8]) -> u8 {
    let mut crc = CRC8_INIT;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            if crc & 0x80 != 0 {
                crc = (crc << 1) ^ CRC8_POLYNOMIAL;
            } else {
                crc <<= 1;
            }
        }
    }
    crc
}

fn sensiron_send(device: &I2CDevice, data: u16) -> bool {
    let mut bytes: [u8;3] = [0;3];
    bytes[..2].copy_from_slice(&data.to_be_bytes());
    bytes[2] = sensirion_common_generate_crc(&bytes[0..2]);
    let pointer_bytes = bytes.as_ptr();
    let bytes_written = unsafe {
        libc::write(device.fd, pointer_bytes as *const libc::c_void, bytes.len())
    };

    bytes_written == 3
}

async fn sensiron_read_u16<const COUNT: usize>(device: &I2CDevice) -> std::result::Result<[u16;COUNT], String> where [(); COUNT*3]: {
    let bytes: [u8;COUNT*3] = [0;COUNT*3];
    let pointer_bytes = bytes.as_ptr();
    let bytes_read:usize = unsafe {
        libc::read(device.fd, pointer_bytes as *mut libc::c_void, 9)
    }.try_into().unwrap();

    if bytes_read != COUNT*3 {
        return Err("Read not complete".to_string());
    }

    let mut result: [u16;COUNT] = [0;COUNT];

    for i in 0..COUNT {
        let bound = (i+1)*3-1;
        let expected_crc = sensirion_common_generate_crc(&bytes[(i*3)..bound]);

        if expected_crc != bytes[bound] {
            return Err("Bad CRC".to_string());
        }
        let data_bytes: [u8;2] = bytes[i*3..bound].try_into().unwrap();
        result[i] = u16::from_be_bytes(data_bytes);

    }

    Ok(result)
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct Labels {
    room: String,
}


#[derive(Clone)]
struct State {
    registry: Arc<Registry>,
}

async fn read_scd40(co2_metric: Family<Labels,Gauge>, temp_c_metric: Family<Labels,Gauge<f64, AtomicU64>>,
                    rh_percent_metric: Family<Labels,Gauge<f64, AtomicU64>>) {
    let room = String::from("Hobby room");
    let label = Labels{room:room};


    let device = I2CDevice::new(&BUS,&SCD40_ADDRESS).expect("failed to get device");
    let session = SCD40Session::new(device).await.expect("failed to start session");

    loop {
        // TODO: make async

        sensiron_send(&session.device, READ_MEASUREMENT);
        async_std::task::sleep(Duration::from_millis(1)).await;

        let [co2, temp_raw, rh_raw] = sensiron_read_u16::<3>(&session.device).await.expect("cannot read co2");
        let temp = -45.0 + 175.0 * ( temp_raw as f32 / 65536.0);
        let rh = 100.0 * ( rh_raw as f32 / 65536.0);
        co2_metric.get_or_create(&label).set(co2 as i64);
        temp_c_metric.get_or_create(&label).set(temp as f64);
        rh_percent_metric.get_or_create(&label).set(rh as f64);
        //*(state.co2_ppm.lock().unwrap()) = co2 as u32;
        //*(state.temp_c.lock().unwrap()) = temp;
        //*(state.rh_percent.lock().unwrap()) = rh;
        async_std::task::sleep(Duration::from_millis(5100)).await;
    }
}

async fn get_readings(_req: Request<State>) -> tide::Result {
    //Ok(Response::builder(StatusCode::Ok).body(format!("co2: {} ppm, temp: {} C, RH: {} %", 
    //    *(req.state().co2_ppm.lock().unwrap()),
    //    *(req.state().temp_c.lock().unwrap()),
    //    *(req.state().rh_percent.lock().unwrap()))).build())
    Ok(Response::builder(StatusCode::Ok).body(format!("todo")).build())
}

struct PMSA003IReading {
    pm1_0_ug_m3: u16,
    pm2_5_ug_m3: u16,
    pm10_0_ug_m3: u16,

    greater_0_3_ct: u16,
    greater_0_5_ct: u16,
    greater_1_0_ct: u16,
    greater_2_5_ct: u16,
    greater_5_0_ct: u16,
    greater_10_0_ct: u16,
}

impl PMSA003IReading {

    fn new(bytes: &[u8;32]) -> Result<Self,String> {
        if bytes[0] != 0x42 || bytes[1] != 0x4d {
            return Err("Bad reading".to_string());
        }
       let pm1_0_ug_m3_bytes:  [u8;2] = bytes[0x0a..0x0c].try_into().unwrap();
       let pm2_5_ug_m3_bytes:  [u8;2] = bytes[0x0c..0x0e].try_into().unwrap();
       let pm10_0_ug_m3_bytes:  [u8;2] = bytes[0x0e..0x10].try_into().unwrap();
       let greater_0_3_ct_bytes: [u8;2] = bytes[0x10..0x12].try_into().unwrap();
       let greater_0_5_ct_bytes: [u8;2] = bytes[0x12..0x14].try_into().unwrap();
       let greater_1_0_ct_bytes: [u8;2] = bytes[0x14..0x16].try_into().unwrap();
       let greater_2_5_ct_bytes: [u8;2] = bytes[0x16..0x18].try_into().unwrap();
       let greater_5_0_ct_bytes: [u8;2] = bytes[0x18..0x1a].try_into().unwrap();
       let greater_10_0_ct_bytes: [u8;2] = bytes[0x1a..0x1c].try_into().unwrap();

        Ok(
            PMSA003IReading {
                pm1_0_ug_m3:  u16::from_be_bytes(pm1_0_ug_m3_bytes),
                pm2_5_ug_m3:  u16::from_be_bytes(pm2_5_ug_m3_bytes),
                pm10_0_ug_m3:  u16::from_be_bytes(pm10_0_ug_m3_bytes),
                greater_0_3_ct: u16::from_be_bytes(greater_0_3_ct_bytes),
                greater_0_5_ct: u16::from_be_bytes(greater_0_5_ct_bytes),
                greater_1_0_ct: u16::from_be_bytes(greater_1_0_ct_bytes),
                greater_2_5_ct: u16::from_be_bytes(greater_2_5_ct_bytes),
                greater_5_0_ct: u16::from_be_bytes(greater_5_0_ct_bytes),
                greater_10_0_ct: u16::from_be_bytes(greater_10_0_ct_bytes),
            })

    }
}

async fn read_pmsa003i(
    pm1_0_ug_m3_metric: Family<Labels,Gauge>, 
    pm2_5_ug_m3_metric: Family<Labels,Gauge>, 
    pm10_0_ug_m3_metric: Family<Labels,Gauge>, 
    greater_0_3_ct_metric: Family<Labels,Gauge>, 
    greater_0_5_ct_metric: Family<Labels,Gauge>, 
    greater_1_0_ct_metric: Family<Labels,Gauge>, 
    greater_2_5_ct_metric: Family<Labels,Gauge>, 
    greater_5_0_ct_metric: Family<Labels,Gauge>, 
    greater_10_0_ct_metric: Family<Labels,Gauge>, 
    ) {
    let room = String::from("Hobby room");
    let label = Labels{room:room};

    let device = I2CDevice::new(&BUS,&PMSA003I_ADDRESS).expect("failed to get device");

    loop {
        let bytes: [u8;32] = [0;32];
        let pointer_bytes = bytes.as_ptr();
        let bytes_read:usize = unsafe {
            libc::read(device.fd, pointer_bytes as *mut libc::c_void, 32)
            }.try_into().unwrap();
        let reading = PMSA003IReading::new(&bytes).unwrap();

        pm1_0_ug_m3_metric.get_or_create(&label).set(reading.pm1_0_ug_m3 as i64);
        pm2_5_ug_m3_metric.get_or_create(&label).set(reading.pm2_5_ug_m3 as i64);
        pm10_0_ug_m3_metric.get_or_create(&label).set(reading.pm10_0_ug_m3 as i64);

        greater_0_3_ct_metric.get_or_create(&label).set(reading.greater_0_3_ct as i64);
        greater_0_5_ct_metric.get_or_create(&label).set(reading.greater_0_5_ct as i64);
        greater_1_0_ct_metric.get_or_create(&label).set(reading.greater_1_0_ct as i64);
        greater_2_5_ct_metric.get_or_create(&label).set(reading.greater_2_5_ct as i64);
        greater_5_0_ct_metric.get_or_create(&label).set(reading.greater_5_0_ct as i64);
        greater_10_0_ct_metric.get_or_create(&label).set(reading.greater_10_0_ct as i64);
        println!("read");

        async_std::task::sleep(Duration::from_millis(5000)).await;
    }
}


#[async_std::main]
async fn main() -> std::result::Result<(), std::io::Error> {


    //let state: Readings = Readings::new ( 500, 20.0, 50.0)?;

    let mut registry = Registry::default();
    let co2_metric = Family::<Labels, Gauge>::default();
    registry.register_with_unit(
        "co2_concentration",
        "CO2 concentration in PPM",
        Other("ppm".to_string()),
        co2_metric.clone(),
    );
    let temp_c_metric = Family::<Labels, Gauge<f64,AtomicU64>>::default();
    registry.register_with_unit(
        "temperature",
        "The temperature in degrees Celsius",
        Other("C".to_string()),
        temp_c_metric.clone(),
    );
    let rh_metric = Family::<Labels, Gauge<f64,AtomicU64>>::default();
    registry.register_with_unit(
        "relative_humidity",
        "The relative humidity in Percent",
        Other("percent".to_string()),
        rh_metric.clone(),
    );
    let pm1_0_ug_m3_metric = Family::<Labels, Gauge>::default();
    registry.register_with_unit(
        "pm_1_0",
        "Particles smaller than 1um in ug/m3",
        Other("ug_per_m3".to_string()),
        pm1_0_ug_m3_metric.clone(),
    );
    let pm2_5_ug_m3_metric = Family::<Labels, Gauge>::default();
    registry.register_with_unit(
        "pm_2_5",
        "Particles smaller than 2.5um in ug/m3",
        Other("ug_per_m3".to_string()),
        pm2_5_ug_m3_metric.clone(),
    );
    let pm10_0_ug_m3_metric = Family::<Labels, Gauge>::default();
    registry.register_with_unit(
        "pm_10_0",
        "Particles smaller than 10um in ug/m3",
        Other("ug_per_m3".to_string()),
        pm10_0_ug_m3_metric.clone(),
    );
    let greater_0_3_ct_metric= Family::<Labels, Gauge>::default();
    registry.register(
        "greater_0_3_ct",
        "Particles larger than 0.3um in 0.1l of air",
        greater_0_3_ct_metric.clone(),
    );
    let greater_0_5_ct_metric= Family::<Labels, Gauge>::default();
    registry.register(
        "greater_0_5_ct",
        "Particles larger than 0.5um in 0.1l of air",
        greater_0_5_ct_metric.clone(),
    );
    let greater_1_0_ct_metric= Family::<Labels, Gauge>::default();
    registry.register(
        "greater_1_0_ct",
        "Particles larger than 1um in 0.1l of air",
        greater_1_0_ct_metric.clone(),
    );
    let greater_2_5_ct_metric= Family::<Labels, Gauge>::default();
    registry.register(
        "greater_2_5_ct",
        "Particles larger than 2.5um in 0.1l of air",
        greater_2_5_ct_metric.clone(),
    );
    let greater_5_0_ct_metric= Family::<Labels, Gauge>::default();
    registry.register(
        "greater_5_0_ct",
        "Particles larger than 5um in 0.1l of air",
        greater_5_0_ct_metric.clone(),
    );
    let greater_10_0_ct_metric= Family::<Labels, Gauge>::default();
    registry.register(
        "greater_10_0_ct",
        "Particles larger than 10um in 0.1l of air",
        greater_10_0_ct_metric.clone(),
    );

    let state = State {
        registry: Arc::new(registry)};

    task::spawn(async move {
        read_scd40(co2_metric,temp_c_metric,rh_metric).await;
    });

    task::spawn(async move {
        read_pmsa003i(pm1_0_ug_m3_metric,pm2_5_ug_m3_metric, pm10_0_ug_m3_metric,
                      greater_0_3_ct_metric,
                      greater_0_5_ct_metric,
                      greater_1_0_ct_metric,
                      greater_2_5_ct_metric,
                      greater_5_0_ct_metric,
                      greater_10_0_ct_metric).await;
    });

    let mut app = tide::with_state(   state );

    tide::log::start();
    //let mut app = tide::with_state(state);
    app.at("/").get(get_readings);
    app.at("/metrics")
        .get(|req: tide::Request<State>| async move {
            let mut encoded = String::new();
            encode(&mut encoded, &req.state().registry).unwrap();
            let response = tide::Response::builder(200)
                .body(encoded)
                .content_type("application/openmetrics-text; version=1.0.0; charset=utf-8")
                .build();
            Ok(response)
        });
    app.listen("0.0.0.0:9900").await?;
    Ok(())
}
    ////let temp_c = Family::<Labels, Gauge>::default();
    ////let rh = Family::<Labels, Gauge>::default();











