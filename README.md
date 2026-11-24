# Cotinga

**Cotinga** is a command-line G-code optimizer designed to reduce unnecessary pen-up travel in plotter-generated toolpaths.

It works by treating individual drawing strokes as a routing problem, reordering and reversing strokes where beneficial to minimize the distance traveled between them while preserving their geometry. The result is less wasted movement and faster plotting.

## Features

- **Greedy optimization:** Uses nearest-neighbor selection to efficiently construct an initial route.
- **2-opt optimization:** Improves existing routes by eliminating inefficient connections.
- **Direction reversal:** Can reverse stroke traversal where beneficial, including handling clockwise and counterclockwise arc movements.
- **G-code parsing:** Processes linear and circular movements (`G1`, `G2`, and `G3`).
- **Header preservation:** Retains the original G-code setup commands when generating optimized output.
- **Efficient implementation:** Written in Rust to support fast route evaluation and optimization of large toolpaths.

## Usage

```bash
cotinga input.gcode -o optimized.gcode
```

### Options

| Argument | Description |
|---|---|
| `filename` | Input G-code file |
| `-o, --output` | Output file path |
| `-m, --mode` | Optimization algorithm (default: `greedy`) |
| `-z  --zhop` | Z height for travel moves, in the units the GCODE uses (default: `1.0`) |
| `-h, --help` | Display help information |

## Building from Source

Requires the [Rust toolchain](https://www.rust-lang.org/tools/install).

```bash
git clone https://github.com/Tridius1/cotinga.git
cd cotinga
cargo build --release
```

The compiled executable will be available in `target/release/`.

## License

Cotinga is released under the [MIT License](LICENSE).

Copyright (c) 2026 Tristan Swanson.
