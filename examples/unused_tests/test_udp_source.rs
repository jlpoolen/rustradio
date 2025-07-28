// examples/test_udp_source.rs
//
// 2025-07-07 ChatGPT
// $Header$
//

use rustradio::blocks::udp_source::UdpSourceBuilder;

fn main() {
    let block = UdpSourceBuilder {
        bind_addr: "0.0.0.0:5000".to_string(),
        multicast_addr: Some("239.192.0.1".to_string()),
        iface: Some("192.168.1.2".to_string()),
        gain: None,
        reuse_addr: Some(true),
    }
    .build()
    .expect("Failed to build UDP source");

    println!("UDP source block built successfully: {:?}", block.meta().name);
}
