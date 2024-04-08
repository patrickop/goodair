#![feature(generic_const_exprs)]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::ffi::CString;
use std::thread::sleep;
use std::time::Duration;
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
    fn new(bus_path: &str, address: &u64) -> Result<Self, std::io::Error> {
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
    fn new(device: I2CDevice) -> Result<Self, String> {
        println!("sending start measurement");
        if !sensiron_send(&device,START_PERIODIC_MEASUREMENT) {
            return Err("failed to start session".to_string());
        }
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

fn sensiron_read_u16<const COUNT: usize>(device: &I2CDevice) -> Result<[u16;COUNT], String> where [(); COUNT*3]: {
    let bytes: [u8;COUNT*3] = [0;COUNT*3];
    let pointer_bytes = bytes.as_ptr();
    let bytes_read = unsafe {
        libc::read(device.fd, pointer_bytes as *mut libc::c_void, 9)
    };

    if bytes_read != (COUNT*3).try_into().unwrap() {
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




fn main() {
    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    // Set the signal handler
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    }).expect("Error setting Ctrl-C handler");

    let bus = "/dev/i2c-1";

    let device = I2CDevice::new(&bus,&SCD40_ADDRESS).expect("failed to get device");
    let session = SCD40Session::new(device).expect("failed to start session");

    while running.load(Ordering::SeqCst) {
        sleep(Duration::from_millis(5100));

        sensiron_send(&session.device, READ_MEASUREMENT);
        sleep(Duration::from_millis(1));

        let [co2, temp_raw, rh_raw] = sensiron_read_u16::<3>(&session.device).expect("cannot read co2");
        let temp = -45.0 + 175.0 * ( temp_raw as f32 / 65536.0);
        let rh = 100.0 * ( rh_raw as f32 / 65536.0);
        println!("CO2: {} ppm, Temp: {} C, RH: {} %", co2, temp, rh);
    }







}

