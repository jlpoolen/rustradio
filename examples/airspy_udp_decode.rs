/*
clear; date; cargo run  --example airspy_udp_decode --features audio,logging

*/
use anyhow::Result;
use clap::{Arg, ArgAction, Command, Parser};
use log::warn;

use std::borrow::Cow;
//use std::net::IpAddr;
use std::path::{Path, PathBuf};

//use tracing::{debug, error, trace};

use num_complex::Complex;
use rustradio::blocks::*;
use rustradio::graph::GraphRunner;
use rustradio::mtgraph::MTGraph;
use rustradio::parse_verbosity;
use rustradio::udp_source::UdpSourceBuilder;
use rustradio::{ComplexI16, Float, blockchain};

#[derive(clap::Parser, Debug)]
#[command(version, about)]
struct Opt {
    /// Input file in airspy format (I/Q s16)
    #[arg(short)]
    input: Option<String>,

    #[arg(short, value_parser=parse_verbosity, default_value="info")]
    verbose: usize,

    #[arg(long = "volume", default_value = "0.1")]
    volume: Float,
        #[arg(short, long)]
    multicast_addr: String,

    #[arg(short, long, required_unless_present_any=["port_local", "port_multicast"])]
    port: Option<u16>,

    #[arg(long, requires = "port_multicast")]
    port_local: Option<u16>,

    #[arg(long, requires = "port_local")]
    port_multicast: Option<u16>,

    #[arg(long)]
    iface_addr: Option<String>,

    #[arg(long, default_value = "true")]
    reuse: bool,

    #[arg(long, default_value = "2500000")]
    samp_rate: u32,

    #[arg(long, default_value = "48000")]
    audio_rate: u32,

    #[arg(long)]
    filename: Option<String>,
}

pub fn main() -> Result<()> {
    //tracing_subscriber::fmt::init();

    let opt = Opt::parse();

    // Use multicast_addr directly
    let multicast_addr = opt.multicast_addr.clone();

    // Handle port logic
    let port_local: u16;
    let port_multicast: u16;

    if let Some(p) = opt.port {
        port_local = p;
        port_multicast = p;
    } else {
        port_local = opt.port_local.ok_or_else(|| {
            anyhow::anyhow!("Must provide --port or both --port_local and --port_multicast")
        })?;
        port_multicast = opt.port_multicast.ok_or_else(|| {
            anyhow::anyhow!("Must provide both --port_local and --port_multicast")
        })?;
    }

    // Determine interface address
    let iface_addr = if let Some(addr) = &opt.iface_addr {
        addr.clone()
    } else {
        local_ip_address::local_ip()
            .map_err(|_| anyhow::anyhow!("Failed to auto-detect interface IP"))?
            .to_string()
    };

    // Use reuse as boolean already parsed by Clap
    let reuse_addr = opt.reuse;

    // Sample and audio rates are already properly typed
    let samp_rate = opt.samp_rate;
    let audio_rate = opt.audio_rate;

    // Audio output flag and filename
    let (output_audio, file_name): (bool, Option<String>) = match &opt.filename {
        Some(f) => {
            let path = std::path::Path::new(f);
            if !path.exists() || path.is_file() {
                (false, Some(f.clone()))
            } else {
                return Err(anyhow::anyhow!("Invalid output file path: {f}"));
            }
        }
        None => (true, None),
    };

    // Display for verification
    println!("airspy udp decode");
    println!("Using interface: {}", iface_addr);
    println!(
        "Multicast group: {}, Ports: local={}, multicast={}",
        multicast_addr, port_local, port_multicast
    );
    println!("Sample rate: {}, Audio rate: {}", samp_rate, audio_rate);
    println!("Reuse addr: {}", reuse_addr);


    let verbosity = if cfg!(debug_assertions) {
        opt.verbose
    } else {
        0 // suppress info/debug/trace in release builds
    };
    stderrlog::new()
        .module(module_path!())
        .module("rustradio")
        .quiet(false)
        .verbosity(verbosity)
        .timestamp(stderrlog::Timestamp::Second)
        .init()?;

    let mut g = MTGraph::new();
    let samp_rate = 2_500_000f32;
    let audio_rate = 48000;

    let prev = blockchain![
        g,
        prev,
        UdpSourceBuilder::<ComplexI16>::new(
            "0.0.0.0",     // bind address
            5000,          // local port
            "239.192.0.1", // multicast group
            5000           // multicast port
        )
        .iface_addr(&iface_addr) // ← your NIC's IP
        .reuse_addr(true) 
        .build()?,
        Map::new(prev, "i16 to f32 complex", |x: ComplexI16, tags| {
            (
                Complex::new(x.re as f32, x.im as f32) / 1000.0,
                Cow::Borrowed(tags),
            )
        }),
        FftFilter::new(
            prev,
            rustradio::fir::low_pass_complex(
                samp_rate,
                12_500.0,
                10_000.0,
                &rustradio::window::WindowType::Hamming,
            )
        ),
        Map::keep_tags(prev, "am decode", |v| v.norm()),
        FftFilterFloat::new(
            prev,
            &rustradio::fir::low_pass(
                samp_rate,
                audio_rate as Float,
                500.0,
                &rustradio::window::WindowType::Hamming,
            )
        ),
        RationalResampler::builder()
            .deci(samp_rate as usize)
            .interp(audio_rate)
            .build(prev)?,
        MultiplyConst::new(prev, opt.volume),
    ];

    if output_audio {
        g.add(Box::new(AudioSink::new(prev, audio_rate as u64)?));
    } else {
        let file_path = PathBuf::from(file_name.as_ref().unwrap());
        g.add(Box::new(FileSink::new(
            prev,
            file_path,
            //rustradio::file_sink::Mode::Overwrite,
            rustradio::file_sink::Mode::Append,
        )?));
    }

    let cancel = g.cancel_token();
    ctrlc::set_handler(move || {
        warn!("Got Ctrl-C");
        eprintln!("\n");
        cancel.cancel();
    })
    .expect("failed to set Ctrl-C handler");
    eprintln!("Running loop");
    g.run()?;
    eprintln!("{}", g.generate_stats().unwrap());
    Ok(())
}
