use cloudy_af_lib::firmware::flasher::{open_device, read_monitoring_data};
use std::thread;
use std::time::Duration;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Connecting to device...");
    let mut device = open_device()?;
    
    println!("Connected. Polling telemetry (Ctrl+C to stop)...");
    println!("Format: 8x8 grid of bytes");
    println!("--------------------------------------------------");

    loop {
        match read_monitoring_data(&mut device) {
            Ok(data) => {
                let mut output = String::new();
                for row in 0..8 {
                    for col in 0..8 {
                        output.push_str(&format!("{:02x} ", data[row * 8 + col]));
                    }
                    output.push('\n');
                }
                
                print!("\x1B[2J\x1B[H"); 
                println!("Live Telemetry Dump:");
                println!("{}", output);
            }
            Err(e) => {
                eprintln!("Error reading telemetry: {}", e);
            }
        }
        
        thread::sleep(Duration::from_millis(100));
    }
}
