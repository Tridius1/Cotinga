
use crate::movement::{ArcMove, LinearMove, Move, Point, Stroke};

#[derive(Debug, Clone)]
pub struct GCode {
    pub header: Vec<String>,
    pub strokes: Vec<Stroke>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PositioningMode {
    Unknown,
    Absolute,
    Relative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Units {
    Unknown,
    Inches,
    Millimeters,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MotionMode {
    Rapid,
    Linear,
    ClockwiseArc,
    CounterclockwiseArc,
}

#[derive(Debug, Default)]
struct Words {
    motion: Option<MotionMode>,
    positioning: Option<PositioningMode>,
    units: Option<Units>,

    x: Option<f64>,
    y: Option<f64>,
    z: Option<f64>,

    i: Option<f64>,
    j: Option<f64>,

    feed_rate: Option<f64>,
}

#[derive(Debug)]
struct ParserState {
    positioning: PositioningMode,
    units: Units,
    motion: Option<MotionMode>,
    feed_rate: Option<f64>,
    position: Option<Point>,
}

impl Default for ParserState {
    fn default() -> Self {
        Self {
            positioning: PositioningMode::Unknown,
            units: Units::Unknown,
            motion: None,
            feed_rate: None,
            position: None,
        }
    }
}

impl ParserState {
    fn update(
        &mut self,
        words: &Words,
        started: bool,
    ) -> Result<(), String> {
        if let Some(positioning) = words.positioning {
            self.positioning = positioning;
        }

        if self.positioning == PositioningMode::Relative {
            return Err("Relative positioning (G91) is not supported.".into());
        }

        if let Some(units) = words.units {
            if started && self.units != units {
                return Err("Changing units during a toolpath is not supported.".into());
            }

            self.units = units;
        }

        if let Some(motion) = words.motion {
            self.motion = Some(motion);
        }

        if let Some(feed_rate) = words.feed_rate {
            if feed_rate <= 0.0 {
                return Err(format!("Invalid feed rate: {feed_rate}"));
            }

            self.feed_rate = Some(feed_rate);
        }

        Ok(())
    }

    fn update_position(&mut self, words: &Words) -> Result<Point, String> {
        if self.positioning != PositioningMode::Absolute {
            return Err("G90 must be specified before XY movement.".into());
        }

        let current = self.position.ok_or_else(|| {
            "Initial XY position is unknown.".to_string()
        })?;

        let next = Point::new(
            words.x.unwrap_or(current.x),
            words.y.unwrap_or(current.y),
        );

        if !next.x.is_finite() || !next.y.is_finite() {
            return Err("Invalid XY coordinates.".into());
        }

        self.position = Some(next);
        Ok(next)
    }
}


pub fn parse_gcode(input: &str) -> Result<GCode, String> {
    let mut state = ParserState::default();

    let mut header = Vec::new();
    let mut strokes = Vec::new();

    let mut points = Vec::new();
    let mut moves = Vec::new();

    let mut started = false;

    for (index, line) in input.lines().enumerate() {
        let line_number = index + 1;

        let words = parse_words(line)
            .map_err(|e| format!("Line {line_number}: {e}"))?;

        state.update(&words, started)
            .map_err(|e| format!("Line {line_number}: {e}"))?;

        let has_xy = words.x.is_some() || words.y.is_some();
        let has_motion = has_xy || words.i.is_some() || words.j.is_some();

        // Find the initial machine position.
        if !started {
            if state.position.is_none() {
                header.push(line.to_string());

                if has_xy {
                    if state.positioning != PositioningMode::Absolute {
                        return Err(format!(
                            "Line {line_number}: G90 required before XY movement."
                        ));
                    }

                    if state.units == Units::Unknown {
                        return Err(format!(
                            "Line {line_number}: G20 or G21 required before XY movement."
                        ));
                    }

                    let position = Point::new(
                        words.x.ok_or_else(|| {
                            format!("Line {line_number}: Initial X coordinate missing.")
                        })?,
                        words.y.ok_or_else(|| {
                            format!("Line {line_number}: Initial Y coordinate missing.")
                        })?,
                    );

                    state.position = Some(position);
                }

                continue;
            }

            // Preserve additional initialization and Z-only movements.
            if state.motion == Some(MotionMode::Rapid) || !has_motion {
                header.push(line.to_string());

                if has_xy {
                    state.update_position(&words)
                        .map_err(|e| format!("Line {line_number}: {e}"))?;
                }

                continue;
            }
        }

        let motion = state.motion.ok_or_else(|| {
            format!("Line {line_number}: Missing motion mode.")
        })?;

        // Rapid movements separate strokes.
        if motion == MotionMode::Rapid {
            flush_stroke(&mut strokes, &mut points, &mut moves)?;

            if has_xy {
                state.update_position(&words)
                    .map_err(|e| format!("Line {line_number}: {e}"))?;
            }

            continue;
        }

        if !has_motion {
            continue;
        }

        let start = state.position.ok_or_else(|| {
            format!("Line {line_number}: Unknown starting position.")
        })?;

        let end = state.update_position(&words)
            .map_err(|e| format!("Line {line_number}: {e}"))?;

        if points.is_empty() {
            points.push(start);
        }

        let movement = match motion {
            MotionMode::Linear => {
                Move::Linear(LinearMove::new(state.feed_rate))
            }

            MotionMode::ClockwiseArc
            | MotionMode::CounterclockwiseArc => {
                let center = Point::new(
                    start.x + words.i.unwrap_or(0.0),
                    start.y + words.j.unwrap_or(0.0),
                );

                let arc = ArcMove::new(state.feed_rate, center);

                match motion {
                    MotionMode::ClockwiseArc => Move::ClockwiseArc(arc),
                    MotionMode::CounterclockwiseArc => Move::CounterclockwiseArc(arc),
                    _ => unreachable!(),
                }
            }

            MotionMode::Rapid => unreachable!(),
        };

        moves.push(movement);
        points.push(end);

        started = true;
    }

    flush_stroke(&mut strokes, &mut points, &mut moves)?;

    if strokes.is_empty() {
        return Err("No cutting movements found.".into());
    }

    Ok(GCode {
        header,
        strokes,
    })
}


fn flush_stroke(
    strokes: &mut Vec<Stroke>,
    points: &mut Vec<Point>,
    moves: &mut Vec<Move>,
) -> Result<(), String> {
    if moves.is_empty() {
        points.clear();
        return Ok(());
    }

    strokes.push(Stroke::new(
        std::mem::take(points),
        std::mem::take(moves),
    )?);

    Ok(())
}


fn parse_words(line: &str) -> Result<Words, String> {
    // Remove semicolon comments.
    let line = line.split_once(';').map_or(line, |(code, _)| code);

    // Remove parenthetical comments.
    let mut cleaned = String::with_capacity(line.len());
    let mut depth = 0usize;

    for ch in line.chars() {
        match ch {
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            _ if depth == 0 => cleaned.push(ch),
            _ => {}
        }
    }

    let bytes = cleaned.as_bytes();
    let mut words = Words::default();
    let mut index = 0;

    while index < bytes.len() {
        let letter = bytes[index].to_ascii_uppercase();

        if !letter.is_ascii_alphabetic() {
            index += 1;
            continue;
        }

        index += 1;

        // Skip whitespace between a word and its value.
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }

        let start = index;

        // Optional sign.
        if index < bytes.len()
            && matches!(bytes[index], b'+' | b'-')
        {
            index += 1;
        }

        // Integer portion.
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }

        // Optional fractional portion.
        if index < bytes.len() && bytes[index] == b'.' {
            index += 1;

            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
        }

        // Optional scientific notation.
        if index < bytes.len()
            && matches!(bytes[index], b'e' | b'E')
            && index > start
        {
            let exponent_start = index;
            index += 1;

            if index < bytes.len()
                && matches!(bytes[index], b'+' | b'-')
            {
                index += 1;
            }

            let digits_start = index;

            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }

            if digits_start == index {
                index = exponent_start;
            }
        }

        if start == index {
            // Ignore unknown words without numeric values,
            // such as text in commands we don't interpret.
            continue;
        }

        let value: f64 = cleaned[start..index]
            .parse()
            .map_err(|_| format!("Invalid value for {}.", letter as char))?;

        if !value.is_finite() {
            return Err(format!(
                "Non-finite value for {}.",
                letter as char
            ));
        }

        match letter {
            b'G' => match value {
                0.0 => words.motion = Some(MotionMode::Rapid),
                1.0 => words.motion = Some(MotionMode::Linear),
                2.0 => words.motion = Some(MotionMode::ClockwiseArc),
                3.0 => words.motion = Some(MotionMode::CounterclockwiseArc),

                20.0 => words.units = Some(Units::Inches),
                21.0 => words.units = Some(Units::Millimeters),

                90.0 => words.positioning = Some(PositioningMode::Absolute),
                91.0 => words.positioning = Some(PositioningMode::Relative),

                _ => {}
            },

            b'X' => words.x = Some(value),
            b'Y' => words.y = Some(value),
            b'Z' => words.z = Some(value),

            b'I' => words.i = Some(value),
            b'J' => words.j = Some(value),

            b'F' => words.feed_rate = Some(value),

            // N, M, E, S, T, and other words are ignored.
            _ => {}
        }
    }

    Ok(words)
}



use crate::movement::RouteStep;

/// Generate G-code from an optimized route.
pub fn generate_gcode(
    header: &[String],
    strokes: &[Stroke],
    route: &[RouteStep],
) -> Result<String, String> {
    let mut output = String::new();

    // Preserve the original header.
    for line in header {
        output.push_str(line);
        output.push('\n');
    }

    for step in route {
        let stroke = strokes.get(step.stroke_idx).ok_or_else(|| {
            format!("Invalid stroke index: {}", step.stroke_idx)
        })?;

        let entry = stroke.entry(step.inverted);

        // Rapid positioning to the stroke's starting point.
        output.push_str(&format!(
            "G0 X{} Y{}\n",
            entry.x, entry.y
        ));

        // Lower pen
        output.push_str("G0 Z0\n");

        for (index, _movement) in stroke.moves_iter(step.inverted).enumerate() {
            let end = if step.inverted {
                stroke.points()[stroke.points().len() - 2 - index]
            } else {
                stroke.points()[index + 1]
            };

            let movement = stroke.effective_move(index, step.inverted)
                .ok_or_else(|| "Failed to retrieve movement.".to_string())?;

            match movement {
                Move::Linear(movement) => {
                    output.push_str(&format!(
                        "G1 X{} Y{}",
                        end.x, end.y
                    ));

                    if let Some(feed) = movement.feed_rate {
                        output.push_str(&format!(" F{}", feed));
                    }

                    output.push('\n');
                }

                Move::ClockwiseArc(movement)
                | Move::CounterclockwiseArc(movement) => {
                    let (i, j) = (
                        movement.center.x - stroke.points_iter(step.inverted)
                            .nth(index)
                            .unwrap().x,
                        movement.center.y - stroke.points_iter(step.inverted)
                            .nth(index)
                            .unwrap().y,
                    );

                    let command = if matches!(
                        stroke.effective_move(index, step.inverted),
                        Some(Move::ClockwiseArc(_))
                    ) {
                        "G2"
                    } else {
                        "G3"
                    };

                    output.push_str(&format!(
                        "{} X{} Y{} I{} J{}",
                        command, end.x, end.y, i, j
                    ));

                    if let Some(feed) = movement.feed_rate {
                        output.push_str(&format!(" F{}", feed));
                    }

                    output.push('\n');
                }
            }
        }
        // Lift the pen before moving to another stroke.
        output.push_str("G0 Z1\n");
    }

    Ok(output)
}
