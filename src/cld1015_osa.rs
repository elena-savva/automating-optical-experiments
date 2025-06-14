use std::fs::{self, File, create_dir_all};
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::time::Duration;
use visa_rs::prelude::*;
use crate::visa_error::io_to_vs_err;

/// Performs a current sweep with the CLD1015 laser diode 
/// and captures spectral data from the HP-70952B optical spectrum analyzer
pub fn run_current_sweep(
    cld1015: &mut Instrument,
    osa: &mut Instrument,
    start_ma: f64,
    stop_ma: f64,
    step_ma: f64,
    dwell_time_ms: u64,
) -> visa_rs::Result<()> {
    // Create a CSV file to save summary results
    std::fs::create_dir_all("data").unwrap_or_else(|e| {
        println!("Warning: Failed to create data directory: {}", e);
    });
    let mut file = File::create("data/current_sweep_results.csv").unwrap();
    writeln!(file, "Current (mA),Peak Wavelength (nm),Peak Power (dBm)").unwrap();
    
    // Create a directory to store trace data files
    let trace_dir = "data/current_sweep_trace_data";
    create_dir_all(trace_dir).unwrap_or_else(|e| {
        println!("Warning: Failed to create trace data directory: {}", e);
    });
    
    // Calculate number of points
    let num_points = ((stop_ma - start_ma) / step_ma).floor() as usize + 1;
    println!("Starting current sweep with {} points", num_points);
    
    // Set the CLD1015 to operate in Constant Current mode
    cld1015.write_all(b"SOURce:FUNCtion:MODE CURRent\n").map_err(io_to_vs_err)?;
    // Set current limit to a safe value
    cld1015.write_all(b"SOURce:CURRent:LIMit:AMPLitude 100MA\n").map_err(io_to_vs_err)?;

    // Configure the OSA for measurements
    osa.write_all(b"SNGLS;\n").map_err(io_to_vs_err)?; // Set to single sweep mode
    osa.write_all(b"CENTERWL 974.7NM;SPANWL 2NM;\n").map_err(io_to_vs_err)?;

    let center_wl = 974.7; // Center wavelength in nm
    let span_wl = 2.0;    // Span in nm
    let start_wl = center_wl - (span_wl / 2.0); 
    let stop_wl = center_wl + (span_wl / 2.0);  

    // Get number of data points in trace
    osa.write_all(b"MDS?;\n").map_err(io_to_vs_err)?;
    let mut mds_response = String::new();
    {
        let mut reader = BufReader::new(&*osa);
        reader.read_line(&mut mds_response).map_err(io_to_vs_err)?;
    }
    let num_trace_points = mds_response.trim().parse::<usize>().unwrap_or(800); // Default 800 if parsing fails
    println!("Trace has {} data points", num_trace_points);
    
    // Turn laser OFF
    cld1015.write_all(b"OUTPut:STATe 0\n").map_err(io_to_vs_err)?;
    println!("Laser turned OFF");

    // Turn TEC on before laser activation
    cld1015.write_all(b"OUTPut2:STATe 1\n").map_err(io_to_vs_err)?;

    // Wait for initial stabilization
    std::thread::sleep(Duration::from_millis(100));
    
    // Turn laser ON
    cld1015.write_all(b"OUTPut:STATe 1\n").map_err(io_to_vs_err)?;
    println!("Laser turned ON");
    
    // Wait for initial stabilization
    std::thread::sleep(Duration::from_millis(100));
    
    // Perform the sweep
    for i in 0..num_points {
        let current_ma = start_ma + (i as f64 * step_ma);
        
        // Convert mA to A for the device
        let current_a = current_ma / 1000.0;
        
        // Set the current
        let cmd = format!("SOURce:CURRent:LEVel:IMMediate:AMPLitude {:.6}\n", current_a);
        cld1015.write_all(cmd.as_bytes()).map_err(io_to_vs_err)?;
        
        println!("Set current to {:.2} mA", current_ma);
        
        // Wait for stabilization
        std::thread::sleep(Duration::from_millis(dwell_time_ms));
        println!("Starting sweep");
        
        // Trigger a new sweep on the OSA and confirm it's done before proceeding
        osa.write_all(b"TS;DONE?;\n").map_err(io_to_vs_err)?; // Take sweep
        let mut done_resp = String::new();
        {
            let mut reader = BufReader::new(&*osa);
            reader.read_line(&mut done_resp).map_err(io_to_vs_err)?;
        }
        if done_resp.trim() != "1" {
            println!("Warning: Sweep not confirmed complete. Response: {}", done_resp.trim());
        }
        
        // Find peak
        osa.write_all(b"MKPK HI;\n").map_err(io_to_vs_err)?; // Mark highest signal level
        
        // Get peak wavelength
        osa.write_all(b"MKWL?;\n").map_err(io_to_vs_err)?;
        let mut peak_wavelength = String::new();
        {
            let mut reader = BufReader::new(&*osa);
            reader.read_line(&mut peak_wavelength).map_err(io_to_vs_err)?;
        }
        let peak_wavelength_nm = peak_wavelength.trim().parse::<f64>().unwrap_or(0.0) * 1.0e9; // Convert from meters to nm
        
        // Get peak amplitude
        osa.write_all(b"MKA?;\n").map_err(io_to_vs_err)?;
        let mut peak_power = String::new();
        {
            let mut reader = BufReader::new(&*osa);
            reader.read_line(&mut peak_power).map_err(io_to_vs_err)?;
        }
        let peak_power_dbm = peak_power.trim().parse::<f64>().unwrap_or(-100.0);
        
        // Print measured values
        println!("  Peak Wavelength: {:.3} nm", peak_wavelength_nm);
        println!("  Peak Power: {:.2} dBm", peak_power_dbm);
        
        // Write to results file
        writeln!(file, "{:.2},{:.4},{:.2}", 
                current_ma, peak_wavelength_nm, peak_power_dbm).unwrap();
        
        // Fetch the entire trace data
        println!("Retrieving trace data...");
        osa.write_all(b"TRA?;\n").map_err(io_to_vs_err)?;
        
        // Read trace data
        let mut current_sweep_trace_data = String::new();
        {
            let mut reader = BufReader::new(&*osa);
            reader.read_line(&mut current_sweep_trace_data).map_err(io_to_vs_err)?;
        }
        
        // Calculate wavelength array for the x-axis
        let wavelength_step = (stop_wl - start_wl) / (num_trace_points as f64 - 1.0);
        
        // Create trace data file
        let trace_filename = format!("{}/trace_{:.2}mA.csv", trace_dir, current_ma);
        let mut trace_file = File::create(&trace_filename).unwrap_or_else(|e| {
            println!("Warning: Failed to create trace file {}: {}", trace_filename, e);
            File::create("trace_data_fallback.csv").unwrap()
        });
        
        // Write header to trace file
        writeln!(trace_file, "Wavelength (nm),Power (dBm)").unwrap();
        
        // Parse and write trace data
        let values: Vec<&str> = current_sweep_trace_data.trim().split(',').collect();
        for (j, value) in values.iter().enumerate() {
            if j < num_trace_points {
                let wavelength = start_wl + (j as f64 * wavelength_step);
                let power = value.parse::<f64>().unwrap_or(-100.0);
                writeln!(trace_file, "{:.4},{:.4}", wavelength, power).unwrap();
            }
        }
        
        println!("  Trace data saved to {}", trace_filename);
    }
    
    // Turn laser OFF
    cld1015.write_all(b"OUTPut:STATe 0\n").map_err(io_to_vs_err)?;
    println!("Laser turned OFF");

    osa.write_all(b"SWEEP OFF;\n").map_err(io_to_vs_err)?; // Turn off

    // Check for errors on CLD1015
    cld1015.write_all(b"SYST:ERR?\n").map_err(io_to_vs_err)?;
    
    let mut response = String::new();
    {
        let mut reader = BufReader::new(&*cld1015);
        reader.read_line(&mut response).map_err(io_to_vs_err)?;
    }
    
    println!("Final error check on CLD1015: {}", response.trim());
    
    // Check for errors on OSA
    osa.write_all(b"XERR?;\n").map_err(io_to_vs_err)?;
    
    let mut response = String::new();
    {
        let mut reader = BufReader::new(&*osa);
        reader.read_line(&mut response).map_err(io_to_vs_err)?;
    }
    
    println!("Final error check on OSA: {}", response.trim());
    
    println!("Current sweep completed successfully");
    println!("Summary results saved to current_sweep_results.csv");
    println!("Trace data saved to {}/trace_*mA.csv files", trace_dir);
    
    Ok(())
}
