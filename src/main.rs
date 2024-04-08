#![feature(generic_const_exprs)]
use std::sync::atomic::{AtomicBool, Ordering};
use atomic_float::AtomicF64;
use std::sync::{Arc,Mutex};
use std::ffi::CString;
use std::thread::sleep;
use std::time::Duration;
use prometheus_client::encoding::EncodeLabelValue;
use prometheus_client::encoding::{text::encode, EncodeLabelSet};
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::family::Family;
use prometheus_client::registry::Registry;
use tide::{Middleware, Next, Request, Result, Response, StatusCode};
use async_std::task;

const CRC8_POLYNOMIAL: u8 = 0x31;
const CRC8_INIT: u8 = 0xFF;
const START_PERIODIC_MEASUREMENT:u16 = 0x21b1;
const READ_MEASUREMENT:u16 = 0xec05;
const GET_STATUS_READY:u16 = 0xe4b8;
const I2C_SLAVE:u64 = 0x0703;
const STOP_PERIODIC_MEASUREMENT:u16 = 0x3f86;
const SCD40_ADDRESS:u64 = 0x62;

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

fn sensiron_read_u16<const COUNT: usize>(device: &I2CDevice) -> std::result::Result<[u16;COUNT], String> where [(); COUNT*3]: {
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
struct Readings {
    co2_ppm: Arc<Mutex<u32>>,
    temp_c: Arc<Mutex<f32>>,
    rh_percent: Arc<Mutex<f32>>,
}

impl Readings {
    fn new(co2_ppm: u32, temp_c: f32, rh_percent: f32) -> std::result::Result<Self, std::io::Error> {
        Ok(Readings {
            co2_ppm: Arc::new(Mutex::new(co2_ppm)),
            temp_c: Arc::new(Mutex::new(temp_c)),
            rh_percent: Arc::new(Mutex::new(rh_percent)),
        })
    }
}

async fn read_scd40(state: Readings) {
    let bus = "/dev/i2c-1";

    let device = I2CDevice::new(&bus,&SCD40_ADDRESS).expect("failed to get device");
    let session = SCD40Session::new(device).await.expect("failed to start session");

    loop {
        // TODO: make async

        sensiron_send(&session.device, READ_MEASUREMENT);
        sleep(Duration::from_millis(1));

        let [co2, temp_raw, rh_raw] = sensiron_read_u16::<3>(&session.device).expect("cannot read co2");
        let temp = -45.0 + 175.0 * ( temp_raw as f32 / 65536.0);
        let rh = 100.0 * ( rh_raw as f32 / 65536.0);
        *(state.co2_ppm.lock().unwrap()) = co2 as u32;
        *(state.temp_c.lock().unwrap()) = temp;
        *(state.rh_percent.lock().unwrap()) = rh;
        async_std::task::sleep(Duration::from_millis(5100)).await;
    }
}

async fn get_readings(req: Request<Readings>) -> tide::Result {
    Ok(Response::builder(StatusCode::Ok).body(format!("co2: {} ppm, temp: {} C, RH: {} %", 
        *(req.state().co2_ppm.lock().unwrap()),
        *(req.state().temp_c.lock().unwrap()),
        *(req.state().rh_percent.lock().unwrap()))).build())
}



#[async_std::main]
async fn main() -> std::result::Result<(), std::io::Error> {

    let state: Readings = Readings::new ( 500, 20.0, 50.0)?;

    let reader_state = state.clone();
    task::spawn(async move {
        read_scd40(reader_state).await;
    });




    tide::log::start();
    let mut app = tide::with_state(state);
    app.at("/").get(get_readings);
    app.listen("goodair.local:9900").await?;
    Ok(())
}
    //let mut registry = Registry::default();
    //let co2_metric = Family::<Labels, Gauge>::default();
    ////let temp_c = Family::<Labels, Gauge>::default();
    ////let rh = Family::<Labels, Gauge>::default();
    //registry.register(
    //    "co2_ppm",
    //    "CO2 concentration in PPM",
    //    co2_metric.clone(),
    //);











