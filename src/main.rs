
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::io::{self, Write};
use std::thread;

use clap::{Parser, ValueEnum};

mod movement;
mod gcode;
mod algorithms;

#[derive(Parser)]
#[command(
    name = "gcode-optimizer",
    about = "Optimize G-code for pen plotting"
)]
struct Args {
    /// Input G-code file
    filename: PathBuf,

    /// Output file path
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Optimization algorithm
    #[arg(short, long, value_enum, default_value_t = Mode::Greedy)]
    mode: Mode,
}

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq)]
enum Mode {
    Greedy,
    //Annealing,
    #[value(name = "2opt")]
    TwoOpt,
}

impl Mode {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Greedy => "greedy",
            //Self::Annealing => "annealing",
            Self::TwoOpt => "2opt",
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Determine output path if one wasn't provided.
    let output = args.output.unwrap_or_else(|| {
        args.filename.with_file_name(format!(
            "{}_opt_{}{}",
            args.filename.file_stem().unwrap_or_default().to_string_lossy(),
            args.mode.as_str(),
            args.filename.extension()
                .map(|ext| format!(".{}", ext.to_string_lossy()))
                .unwrap_or_default()
        ))
    });

    println!("\nInput:  {}", args.filename.display());
    println!("Output: {}", output.display());
    println!("Running in {} mode", args.mode.as_str());

    if args.mode == Mode::TwoOpt {
        println!("Note: Progress is an estimate. 2-Opt runs untill no further optimization is found.")
    }

    // Read and parse G-code.
    let input = fs::read_to_string(&args.filename)?;
    let gcode = gcode::parse_gcode(&input)
        .map_err(|e| format!("Failed to parse G-code: {e}"))?;

    let original_distance = movement::travel_distance(
        &gcode.strokes,
        &(0..gcode.strokes.len())
            .map(|i| movement::RouteStep::new(i, false))
            .collect::<Vec<_>>(),
    );

    // Run optimization.
    let (tx, rx) = mpsc::channel::<f64>();

    let route = thread::scope(|scope| {
        let strokes = &gcode.strokes;

        let handle = scope.spawn(move || {
            match args.mode {
                Mode::Greedy => {
                    algorithms::greedy::run(strokes, |progress| {
                        let _ = tx.send(progress);
                    })
                }
                // Mode::Annealing => TODO,
                Mode::TwoOpt => {
                    let mut route = algorithms::greedy::run(
                        strokes,
                        |progress| {
                            let _ = tx.send(progress * 0.2);
                        },
                    );

                    algorithms::twoopt::run(
                        strokes,
                        &mut route,
                        |progress| {
                            let _ = tx.send(0.2 + progress * 0.8);
                        },
                    );

                    route
                }
            }
        });

        // Receive progress while the worker is running.
        for progress in rx {
            print!("\rProgress: {:6.1}%", progress * 100.0);
            let _ = io::stdout().flush();
        }

        println!();

        handle.join().expect("Optimization thread panicked")
    });


    // Generate optimized G-code.
    let output_gcode = gcode::generate_gcode(
        &gcode.header,
        &gcode.strokes,
        &route,
    ).map_err(|e| format!("Failed to generate G-code: {e}"))?;

    // Write output.
    fs::write(&output, output_gcode)?;


    let optimized_distance = movement::travel_distance(
        &gcode.strokes,
        &route,
    );
    let improvement = (1.0 - (optimized_distance / original_distance)) * 100.0;

    println!("\nOptimization complete!");
    eprintln!("Original travel: {:.2}", original_distance);
    eprintln!("Optimized travel: {:.2}", optimized_distance);
    eprintln!("Improvement: {:.2}%", improvement);
    println!("");

    Ok(())
}

