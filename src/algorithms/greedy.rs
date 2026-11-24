
use crate::movement::{Point, RouteStep, Stroke};

/// Orders strokes using a nearest-neighbor greedy algorithm.
///
/// Each stroke can be traversed in either direction.
/// Assumes the initial position is (0, 0).
///
/// Reports progress as a value between 0.0 and 1.0.
pub fn run<F>(
    strokes: &[Stroke],
    mut report_progress: F,
) -> Vec<RouteStep>
where
    F: FnMut(f64),
{
    let count = strokes.len();

    if count == 0 {
        report_progress(1.0);
        return Vec::new();
    }

    let mut route = Vec::with_capacity(count);
    let mut visited = vec![false; count];

    let mut current = Point { x: 0.0, y: 0.0 };

    // Report approximately 100 progress updates.
    let report_interval = (count / 100).max(1);

    for _ in 0..count {
        let mut best_idx = 0;
        let mut best_inverted = false;
        let mut best_distance = f64::INFINITY;

        // Find the nearest unvisited stroke.
        for (idx, stroke) in strokes.iter().enumerate() {
            if visited[idx] {
                continue;
            }

            let normal_distance =
                current.distance_squared(stroke.entry(false));

            let inverted_distance =
                current.distance_squared(stroke.entry(true));

            let (distance, inverted) =
                if inverted_distance < normal_distance {
                    (inverted_distance, true)
                } else {
                    (normal_distance, false)
                };

            if distance < best_distance {
                best_distance = distance;
                best_idx = idx;
                best_inverted = inverted;
            }
        }

        visited[best_idx] = true;

        route.push(RouteStep::new(best_idx, best_inverted));

        current = strokes[best_idx].exit(best_inverted);

        let completed = route.len();

        if completed % report_interval == 0 || completed == count {
            report_progress(completed as f64 / count as f64);
        }
    }

    route
}
