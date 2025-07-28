use anyhow::Result;
use rustradio::udp_iq_source::{UdpIQConfig, UdpIQSource};
use std::{thread, time::Duration};
use rustradio::block::Block;

// constants copied from iq_guard which currently is running
//const UDP_PORT: u16 = 5000;
//const MCAST_ADDR: &str = "239.192.0.1";

fn main() -> Result<()> {
    eprintln!("[DEBUG] Starting test_udp...");
    let mut src = UdpIQSource::new(UdpIQConfig {
        bind_addr: "0.0.0.0:5000".to_string(),
        multicast_addr: "239.192.0.1".to_string(),
        iface: Some("192.168.1.2".to_string()),
        gain: Some(1.0),
        reuse_addr: Some(true),
    })?;
    eprintln!("[DEBUG] UdpIQSource created");

    for i in 0..5 {
        // Call work() to fill the buffer from the socket
        let _ = src.work();

        let output_stream = src.output_stream();
        if let Ok((buffer, _tags)) = output_stream.read_buf() {
            if !buffer.is_empty() {
                println!("[DEBUG] i={i} First sample: {:?}", buffer[0]);
            } else {
                println!("[DEBUG] i={i} Empty buffer");
            }
        } else {
            println!("[DEBUG] i={i} Error reading buffer");
        }
    
        thread::sleep(Duration::from_millis(100));
        eprintln!("{}", i);
    }


    Ok(())
}
