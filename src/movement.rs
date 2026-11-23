
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Squared Euclidean distance.
    #[inline]
    pub fn distance_squared(self, other: Self) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx * dx + dy * dy
    }

    /// Euclidean distance.
    #[inline]
    pub fn distance(self, other: Self) -> f64 {
        self.distance_squared(other).sqrt()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Move {
    Linear(LinearMove),
    ClockwiseArc(ArcMove),
    CounterclockwiseArc(ArcMove),
}

#[derive(Debug, Clone, PartialEq)]
pub struct LinearMove {
    pub feed_rate: Option<f64>,
}

impl LinearMove {
    pub const fn new(feed_rate: Option<f64>) -> Self {
        Self { feed_rate }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ArcMove {
    pub feed_rate: Option<f64>,
    pub center: Point,
}

impl ArcMove {
    pub const fn new(feed_rate: Option<f64>, center: Point) -> Self {
        Self { feed_rate, center }
    }
}

impl Move {
    #[inline]
    pub fn feed_rate(&self) -> Option<f64> {
        match self {
            Self::Linear(movement) => movement.feed_rate,
            Self::ClockwiseArc(movement) => movement.feed_rate,
            Self::CounterclockwiseArc(movement) => movement.feed_rate,
        }
    }

    /// Returns the movement in the opposite direction.
    pub fn reversed(&self) -> Self {
        match self {
            Self::Linear(movement) => Self::Linear(movement.clone()),
            Self::ClockwiseArc(movement) => {
                Self::CounterclockwiseArc(movement.clone())
            }
            Self::CounterclockwiseArc(movement) => {
                Self::ClockwiseArc(movement.clone())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stroke {
    points: Vec<Point>,
    moves: Vec<Move>,
    start: Point,
    end: Point,
}

impl Stroke {
    pub fn new(
        points: Vec<Point>,
        moves: Vec<Move>,
    ) -> Result<Self, String> {
        if points.len() != moves.len() + 1 {
            return Err(format!(
                "Invalid stroke: {} points but {} moves. \
                 A stroke must have exactly one more point than moves.",
                points.len(),
                moves.len()
            ));
        }

        let start = *points.first().ok_or_else(|| {
            "Invalid stroke: at least one point is required.".to_string()
        })?;

        let end = *points.last().unwrap();

        Ok(Self {
            points,
            moves,
            start,
            end,
        })
    }

    pub fn points(&self) -> &[Point] {
        &self.points
    }

    pub fn moves(&self) -> &[Move] {
        &self.moves
    }

    #[inline]
    pub fn start(&self) -> Point {
        self.start
    }

    #[inline]
    pub fn end(&self) -> Point {
        self.end
    }

    #[inline]
    pub fn entry(&self, inverted: bool) -> Point {
        if inverted { self.end } else { self.start }
    }

    #[inline]
    pub fn exit(&self, inverted: bool) -> Point {
        if inverted { self.start } else { self.end }
    }

    /// Returns points in the requested traversal direction.
    pub fn points_iter(
        &self,
        inverted: bool,
    ) -> impl Iterator<Item = &Point> {
        self.points
            .iter()
            .enumerate()
            .map(move |(i, _)| {
                if inverted {
                    &self.points[self.points.len() - 1 - i]
                } else {
                    &self.points[i]
                }
            })
    }

    /// Returns movements in the requested traversal direction.
    ///
    /// Arc directions are not changed here.
    /// Use effective_move() when generating reversed G-code.
    pub fn moves_iter(
        &self,
        inverted: bool,
    ) -> impl Iterator<Item = &Move> {
        self.moves
            .iter()
            .enumerate()
            .map(move |(i, _)| {
                if inverted {
                    &self.moves[self.moves.len() - 1 - i]
                } else {
                    &self.moves[i]
                }
            })
    }

    /// Returns a movement adjusted for reversed traversal.
    pub fn effective_move(
        &self,
        index: usize,
        inverted: bool,
    ) -> Option<Move> {
        let movement = if inverted {
            self.moves.get(self.moves.len().checked_sub(index + 1)?)
        } else {
            self.moves.get(index)
        }?;

        Some(if inverted {
            movement.reversed()
        } else {
            movement.clone()
        })
    }
}

/// A stroke's position and orientation within an optimization route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteStep {
    pub stroke_idx: usize,
    pub inverted: bool,
}

impl RouteStep {
    pub const fn new(stroke_idx: usize, inverted: bool) -> Self {
        Self {
            stroke_idx,
            inverted,
        }
    }
}
