
use std::fs;
use std::path::PathBuf;
use std::sync::mpsc;
use std::io::{self, Write};
use std::thread;
use std::path::Path;

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

    /// Z height for travel moves
    #[arg(short, long, default_value_t = 1.0)]
    zhop: f64,
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
            "{}_Cotinga_{}{}",
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
        &args.zhop,
    ).map_err(|e| format!("Failed to generate G-code: {e}"))?;

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

    let final_gcode = format!(
        "; GCODE optimized for pen plotting with Cotinga {}\n\
         ; Cotinga by Tristan Swanson\n\
         ;\n\
         ; Original travel distance {:.2} {}\n\
         ; Optimized travel distance {:.2} {}\n\
         ; Improvement: {:.2}%\n\n\n{}",
        env!("CARGO_PKG_VERSION"),
        original_distance,
        gcode.units,
        optimized_distance,
        gcode.units,
        improvement,
        output_gcode
    );

    // Write output.
    fs::write(&output, final_gcode)?;

    print!("Optimized GCODE: ");
    print_hyperlink(&output);

    Ok(())
}

fn print_hyperlink(path: &Path) {
    let absolute_path = std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf());

    let path_str = absolute_path
        .display()
        .to_string()
        .replace('\\', "/")
        .trim_start_matches("//?/")
        .to_string();

    let url = format!("file:///{}", path_str);

    print!("\x1b]8;;{}\x1b\\", url);
    print!("{}", path.display());
    print!("\x1b]8;;\x1b\\");
    println!("\n");
}
