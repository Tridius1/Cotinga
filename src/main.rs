
mod movement;
mod gcode;

use std::{env, fs, process};

use gcode::{parse_gcode, generate_gcode};
use movement::RouteStep;

fn main() {
    let path = env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: cargo run -- <gcode_file>");
        process::exit(1);
    });

    let input = fs::read_to_string(&path).unwrap_or_else(|e| {
        eprintln!("Failed to read '{path}': {e}");
        process::exit(1);
    });

    let gcode = parse_gcode(&input).unwrap_or_else(|e| {
        eprintln!("Failed to parse G-code: {e}");
        process::exit(1);
    });

    println!("Successfully parsed G-code!");
    println!("Header lines: {}", gcode.header.len());
    println!("Strokes: {}", gcode.strokes.len());

    let total_moves: usize = gcode.strokes
        .iter()
        .map(|stroke| stroke.moves().len())
        .sum();

    println!("Total moves: {total_moves}");

    // Create a route preserving the original order and direction.
    let route: Vec<RouteStep> = gcode.strokes
        .iter()
        .enumerate()
        .map(|(index, _)| RouteStep::new(index, false))
        .collect();

    // Generate reconstructed G-code.
    let output = generate_gcode(
        &gcode.header,
        &gcode.strokes,
        &route,
    ).unwrap_or_else(|e| {
        eprintln!("Failed to generate G-code: {e}");
        process::exit(1);
    });

    // Write the result.
    let output_path = "optimized.gcode";

    fs::write(output_path, output).unwrap_or_else(|e| {
        eprintln!("Failed to write output: {e}");
        process::exit(1);
    });

    println!("Generated: {output_path}");
}
