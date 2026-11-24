//! Fast route optimization for strokes with reversible direction.
//!
//! Cargo.toml needs:  rayon = "1"
//!
//! The route is an open path starting at (0, 0):
//!
//!   origin -> stroke[0] -> stroke[1] -> ... -> stroke[n-1]
//!
//! The only cost is the pen-up travel between the exit of one stroke and the
//! entry of the next ("links"). Link `k` runs from the exit of position k-1
//! (or the origin when k == 0) to the entry of position k. Link `n` is a
//! "null link" with zero cost, because the path has a free end.
//!
//! Algorithm:
//!   1. Build a k-d tree over every stroke endpoint and find each endpoint's
//!      nearest neighbors (in parallel).
//!   2. Keep a queue of "active" strokes. For an active stroke, look at its
//!      two links and, for each link endpoint, only try neighbors that are
//!      closer than the link being replaced. Candidates are 2-opt reversals
//!      (which also flip stroke directions) and Or-opt relocations of 1-3
//!      strokes (in either direction).
//!   3. Apply the best move found; re-activate the strokes next to every
//!      changed link. Stop when the queue is empty.
 
use std::collections::VecDeque;
 
use rayon::prelude::*;
 
use crate::movement::{Point, RouteStep, Stroke};
 
/// A move must shorten the route by more than this to be applied.
const MIN_IMPROVEMENT: f64 = 0.001;
 
/// Neighbors stored per endpoint. Larger finds more moves but costs more.
const NEIGHBORS: usize = 500;
 
/// Longest run of strokes that Or-opt will relocate.
const MAX_SEGMENT_LEN: usize = 5;
 
/// Moves that would shift more than this many route positions are skipped.
/// Array reversal is O(length), so this bounds the cost of a single move.
const MAX_MOVE_LEN: usize = 1_000_000;
 
/// Progress never reaches 1.0 until the optimizer has actually finished.
const MAX_RUNNING_PROGRESS: f64 = 0.99;
 
/// Minimum increase before another progress report is emitted.
const MIN_REPORT_STEP: f64 = 0.01;
 
/// Share of the progress bar given to building neighbor lists.
const NEIGHBOR_PHASE_WEIGHT: f64 = 0.05;
 
const NO_NODE: u32 = u32::MAX;
const NO_POSITION: usize = usize::MAX;
const ORIGIN: Point = Point::new(0.0, 0.0);
 
// ---------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------
 
struct ProgressTracker<F: FnMut(f64)> {
    report: F,
    last: f64,
}
 
impl<F: FnMut(f64)> ProgressTracker<F> {
    fn new(report: F) -> Self {
        Self { report, last: 0.0 }
    }
 
    /// Monotonic and throttled; never reports 1.0.
    fn emit(&mut self, value: f64) {
        let value = value.clamp(0.0, MAX_RUNNING_PROGRESS);
 
        if value - self.last >= MIN_REPORT_STEP {
            self.last = value;
            (self.report)(value);
        }
    }
 
    fn finish(&mut self) {
        self.last = 1.0;
        (self.report)(1.0);
    }
}
 
// ---------------------------------------------------------------------
// Spatial index
//
// Node ids: endpoint e (0 = start, 1 = end) of stroke s is node 2*s + e.
// The origin is node 2*strokes.len(). Two nodes with equal `id >> 1`
// belong to the same stroke.
// ---------------------------------------------------------------------
 
#[inline]
fn axis_value(p: Point, axis: usize) -> f64 {
    if axis == 0 { p.x } else { p.y }
}
 
/// Arranges `ids` as an implicit balanced k-d tree: the median of every
/// sub-slice is the splitting node, smaller values on its left.
fn build_kd(ids: &mut [u32], points: &[Point], depth: usize) {
    if ids.len() <= 1 {
        return;
    }
 
    let axis = depth % 2;
    let mid = ids.len() / 2;
 
    ids.select_nth_unstable_by(mid, |&a, &b| {
        axis_value(points[a as usize], axis)
            .total_cmp(&axis_value(points[b as usize], axis))
    });
 
    let (left, rest) = ids.split_at_mut(mid);
    let right = &mut rest[1..];
 
    if left.len() + right.len() > 16_384 {
        rayon::join(
            || build_kd(left, points, depth + 1),
            || build_kd(right, points, depth + 1),
        );
    } else {
        build_kd(left, points, depth + 1);
        build_kd(right, points, depth + 1);
    }
}
 
/// Collects up to NEIGHBORS nearest nodes (squared distance, id), sorted
/// ascending, skipping any node of `query_stroke`.
fn knn_search(
    ids: &[u32],
    points: &[Point],
    query: Point,
    query_stroke: u32,
    depth: usize,
    best: &mut Vec<(f64, u32)>,
) {
    if ids.is_empty() {
        return;
    }
 
    let mid = ids.len() / 2;
    let id = ids[mid];
    let p = points[id as usize];
 
    if id >> 1 != query_stroke {
        let d = query.distance_squared(p);
 
        if best.len() < NEIGHBORS || d < best[best.len() - 1].0 {
            let at = best.partition_point(|e| e.0 <= d);
 
            if best.len() == NEIGHBORS {
                best.pop();
            }
 
            best.insert(at, (d, id));
        }
    }
 
    if ids.len() == 1 {
        return;
    }
 
    let axis = depth % 2;
    let diff = axis_value(query, axis) - axis_value(p, axis);
 
    let left = &ids[..mid];
    let right = &ids[mid + 1..];
 
    let (near, far) = if diff < 0.0 {
        (left, right)
    } else {
        (right, left)
    };
 
    knn_search(near, points, query, query_stroke, depth + 1, best);
 
    if best.len() < NEIGHBORS || diff * diff < best[best.len() - 1].0 {
        knn_search(far, points, query, query_stroke, depth + 1, best);
    }
}
 
/// Returns a flat array: the neighbors of node `v` are
/// `lists[v * NEIGHBORS..][..NEIGHBORS]`, nearest first, padded with NO_NODE.
/// Only nodes with `active[v]` get a list.
fn build_neighbor_lists(
    points: &[Point],
    tree: &[u32],
    active: &[bool],
    mut on_progress: impl FnMut(f64),
) -> Vec<u32> {
    const BLOCK_NODES: usize = 8192;
 
    let total = points.len();
    let mut lists = vec![NO_NODE; total * NEIGHBORS];
 
    for (block_index, block) in
        lists.chunks_mut(BLOCK_NODES * NEIGHBORS).enumerate()
    {
        let first = block_index * BLOCK_NODES;
 
        block.par_chunks_mut(NEIGHBORS).enumerate().for_each_init(
            || Vec::<(f64, u32)>::with_capacity(NEIGHBORS),
            |best, (offset, out)| {
                let node = first + offset;
 
                if !active[node] {
                    return;
                }
 
                best.clear();
 
                knn_search(
                    tree,
                    points,
                    points[node],
                    (node >> 1) as u32,
                    0,
                    best,
                );
 
                for (slot, &(_, id)) in out.iter_mut().zip(best.iter()) {
                    *slot = id;
                }
            },
        );
 
        on_progress(
            ((block_index + 1) * BLOCK_NODES).min(total) as f64
                / total as f64,
        );
    }
 
    lists
}
 
// ---------------------------------------------------------------------
// Tour
// ---------------------------------------------------------------------
 
#[derive(Clone, Copy)]
enum Move {
    /// Reverse positions i..=j and flip each stroke's direction.
    Reverse { i: usize, j: usize },
    /// Move positions a..=b so they sit between positions link-1 and link,
    /// optionally reversing (and flipping) the moved strokes.
    Relocate {
        a: usize,
        b: usize,
        link: usize,
        reversed: bool,
    },
}
 
fn consider(best: &mut Option<(f64, Move)>, delta: f64, mv: Move) {
    let threshold = best.map_or(-MIN_IMPROVEMENT, |(d, _)| d);
 
    if delta < threshold {
        *best = Some((delta, mv));
    }
}
 
struct Tour<'a> {
    route: &'a mut [RouteStep],
    /// Entry/exit point of each position, in its current orientation.
    entries: Vec<Point>,
    exits: Vec<Point>,
    /// Position of each stroke in the route (NO_POSITION if absent).
    pos: Vec<usize>,
    origin_node: u32,
}
 
impl<'a> Tour<'a> {
    #[inline]
    fn len(&self) -> usize {
        self.route.len()
    }
 
    /// Exit of position k-1, or the origin for k == 0. Valid for k <= n.
    #[inline]
    fn prev_exit(&self, k: usize) -> Point {
        if k == 0 { ORIGIN } else { self.exits[k - 1] }
    }
 
    #[inline]
    fn entry_node(&self, k: usize) -> u32 {
        let step = &self.route[k];
        (2 * step.stroke_idx + step.inverted as usize) as u32
    }
 
    #[inline]
    fn exit_node(&self, k: usize) -> u32 {
        let step = &self.route[k];
        (2 * step.stroke_idx + (!step.inverted) as usize) as u32
    }
 
    #[inline]
    fn prev_exit_node(&self, k: usize) -> u32 {
        if k == 0 {
            self.origin_node
        } else {
            self.exit_node(k - 1)
        }
    }
 
    /// Cost of link k; the null link (k == n) is free.
    #[inline]
    fn link_cost(&self, k: usize) -> f64 {
        if k >= self.len() {
            0.0
        } else {
            self.prev_exit(k).distance(self.entries[k])
        }
    }
 
    /// Returns (position, is_entry) for a stroke endpoint node.
    #[inline]
    fn locate(&self, node: u32) -> Option<(usize, bool)> {
        let stroke = (node >> 1) as usize;
        let position = *self.pos.get(stroke)?;
 
        if position == NO_POSITION {
            return None;
        }
 
        let is_entry =
            ((node & 1) == 1) == self.route[position].inverted;
 
        Some((position, is_entry))
    }
 
    /// Finds the best improving move involving the links of `stroke`.
    fn find_best_move(
        &self,
        stroke: usize,
        neighbors: &[u32],
    ) -> Option<(f64, Move)> {
        let n = self.len();
        let k = self.pos[stroke];
        let mut best: Option<(f64, Move)> = None;
 
        // The stroke's incoming link (k) and outgoing link (k + 1).
        for l in k..=k + 1 {
            let a_point = self.prev_exit(l);
            let a_node = self.prev_exit_node(l);
            let link = self.link_cost(l);
 
            // None only for the null link at the end of the path.
            let b_point = if l < n { Some(self.entries[l]) } else { None };
 
            // New links must be shorter than the one they replace. The
            // null link costs nothing, so it gets no pruning.
            let bound = if l < n { link } else { f64::INFINITY };
            let bound_sq = bound * bound;
 
            for t1_is_a in [true, false] {
                let (t1_node, t1_point) = if t1_is_a {
                    (a_node, a_point)
                } else if let Some(bp) = b_point {
                    (self.entry_node(l), bp)
                } else {
                    continue;
                };
 
                let list = &neighbors[t1_node as usize * NEIGHBORS..]
                    [..NEIGHBORS];
 
                for &c in list {
                    if c == NO_NODE {
                        break;
                    }
 
                    let Some((q, c_is_entry)) = self.locate(c) else {
                        continue;
                    };
 
                    let c_point = if c_is_entry {
                        self.entries[q]
                    } else {
                        self.exits[q]
                    };
 
                    let d_sq = t1_point.distance_squared(c_point);
 
                    // Neighbors are sorted by distance.
                    if d_sq >= bound_sq {
                        break;
                    }
 
                    let d = d_sq.sqrt();
 
                    // ---- 2-opt -------------------------------------
                    //
                    // Removes links l and m, adds (t1, c) and the link
                    // joining the other two ends.
                    if t1_is_a && !c_is_entry {
                        // c is the exit of position q = m - 1.
                        let m = q + 1;
 
                        if m != l {
                            let other = match b_point {
                                Some(bp) if m < n => {
                                    self.entries[m].distance(bp)
                                }
                                _ => 0.0,
                            };
 
                            let delta =
                                d + other - link - self.link_cost(m);
 
                            let lo = l.min(m);
                            let hi = l.max(m) - 1;
 
                            if hi - lo < MAX_MOVE_LEN {
                                consider(
                                    &mut best,
                                    delta,
                                    Move::Reverse { i: lo, j: hi },
                                );
                            }
                        }
                    } else if !t1_is_a && c_is_entry {
                        // c is the entry of position m.
                        let m = q;
 
                        if m != l {
                            let delta = d
                                + a_point.distance(self.prev_exit(m))
                                - link
                                - self.link_cost(m);
 
                            let lo = l.min(m);
                            let hi = l.max(m) - 1;
 
                            if hi - lo < MAX_MOVE_LEN {
                                consider(
                                    &mut best,
                                    delta,
                                    Move::Reverse { i: lo, j: hi },
                                );
                            }
                        }
                    }
 
                    // ---- Or-opt ------------------------------------
                    //
                    // Moves a segment [sa, sb] containing c's stroke
                    // into link l, so that c ends up next to t1.
                    // x/y are the segment's first/last points after
                    // insertion.
                    for len in 1..=MAX_SEGMENT_LEN {
                        let (sa, sb, x, y, reversed) =
                            match (t1_is_a, c_is_entry) {
                                (true, true) => {
                                    let sb = q + len - 1;
                                    if sb >= n {
                                        break;
                                    }
                                    (q, sb, c_point, self.exits[sb], false)
                                }
                                (true, false) => {
                                    if q + 1 < len {
                                        break;
                                    }
                                    let sa = q + 1 - len;
                                    (sa, q, c_point, self.entries[sa], true)
                                }
                                (false, false) => {
                                    if q + 1 < len {
                                        break;
                                    }
                                    let sa = q + 1 - len;
                                    (sa, q, self.entries[sa], c_point, false)
                                }
                                (false, true) => {
                                    let sb = q + len - 1;
                                    if sb >= n {
                                        break;
                                    }
                                    (q, sb, self.exits[sb], c_point, true)
                                }
                            };
 
                        // The insertion link must not touch the segment.
                        if l >= sa && l <= sb + 1 {
                            continue;
                        }
 
                        let shift = if l > sb { l - sb - 1 } else { sa - l };
 
                        if shift > MAX_MOVE_LEN {
                            continue;
                        }
 
                        let (new_a, new_b) = if t1_is_a {
                            let tail = match b_point {
                                Some(bp) => y.distance(bp),
                                None => 0.0,
                            };
                            (d, tail)
                        } else {
                            (a_point.distance(x), d)
                        };
 
                        // Joins the gap the segment leaves behind.
                        let bridge = if sb + 1 < n {
                            self.prev_exit(sa)
                                .distance(self.entries[sb + 1])
                        } else {
                            0.0
                        };
 
                        let delta = new_a + new_b + bridge
                            - link
                            - self.link_cost(sa)
                            - self.link_cost(sb + 1);
 
                        consider(
                            &mut best,
                            delta,
                            Move::Relocate {
                                a: sa,
                                b: sb,
                                link: l,
                                reversed,
                            },
                        );
                    }
                }
            }
        }
 
        best
    }
 
    /// Stroke ids adjacent to every link that `mv` will change.
    fn touched(&self, mv: Move, out: &mut Vec<usize>) {
        out.clear();
 
        let n = self.len();
        let mut add = |p: Option<usize>| {
            if let Some(p) = p {
                if p < n {
                    out.push(self.route[p].stroke_idx);
                }
            }
        };
 
        match mv {
            Move::Reverse { i, j } => {
                add(i.checked_sub(1));
                add(Some(i));
                add(Some(j));
                add(Some(j + 1));
            }
            Move::Relocate { a, b, link, .. } => {
                add(a.checked_sub(1));
                add(Some(b + 1));
                add(link.checked_sub(1));
                add(Some(link));
 
                for p in a..=b {
                    add(Some(p));
                }
            }
        }
    }
 
    /// Reverses positions i..=j and flips each stroke's direction.
    /// Does not update `pos`.
    fn flip_range(&mut self, i: usize, j: usize) {
        self.route[i..=j].reverse();
 
        for step in &mut self.route[i..=j] {
            step.inverted = !step.inverted;
        }
 
        self.entries[i..=j].reverse();
        self.exits[i..=j].reverse();
 
        for k in i..=j {
            std::mem::swap(&mut self.entries[k], &mut self.exits[k]);
        }
    }
 
    fn refresh_pos(&mut self, lo: usize, hi: usize) {
        for k in lo..=hi {
            self.pos[self.route[k].stroke_idx] = k;
        }
    }
 
    fn apply(&mut self, mv: Move) {
        match mv {
            Move::Reverse { i, j } => {
                self.flip_range(i, j);
                self.refresh_pos(i, j);
            }
            Move::Relocate { a, b, link, reversed } => {
                let seg = b - a + 1;
 
                let (lo, hi, seg_start) = if link > b + 1 {
                    // Segment moves later: [segment][middle] -> [middle][segment].
                    let hi = link - 1;
 
                    self.route[a..=hi].rotate_left(seg);
                    self.entries[a..=hi].rotate_left(seg);
                    self.exits[a..=hi].rotate_left(seg);
 
                    (a, hi, link - seg)
                } else {
                    // Segment moves earlier: [middle][segment] -> [segment][middle].
                    let lo = link;
 
                    self.route[lo..=b].rotate_right(seg);
                    self.entries[lo..=b].rotate_right(seg);
                    self.exits[lo..=b].rotate_right(seg);
 
                    (lo, b, lo)
                };
 
                if reversed {
                    self.flip_range(seg_start, seg_start + seg - 1);
                }
 
                self.refresh_pos(lo, hi);
            }
        }
    }
 
    /// Total pen-up distance. Only used to check move deltas in tests.
    #[cfg(test)]
    fn length(&self) -> f64 {
        (0..self.len()).map(|k| self.link_cost(k)).sum()
    }
}
 
// ---------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------
 
/// Optimizes stroke order and direction to reduce pen-up travel.
///
/// Assumes the pen starts at (0, 0). The route is open: there is no return
/// to the start. The route must contain each stroke at most once.
///
/// If any stroke endpoint is not finite, the route is left unchanged.
///
/// Reports estimated progress as a monotonic value between 0.0 and 1.0.
/// A final value of exactly 1.0 is always reported on completion.
pub fn run<F>(
    strokes: &[Stroke],
    route: &mut [RouteStep],
    report_progress: F,
)
where
    F: FnMut(f64),
{
    let mut progress = ProgressTracker::new(report_progress);
    let n = route.len();
 
    if n == 0 {
        progress.finish();
        return;
    }
 
    let stroke_count = strokes.len();
    let origin_node = (2 * stroke_count) as u32;
 
    // Where each stroke currently sits in the route.
    let mut pos = vec![NO_POSITION; stroke_count];
 
    for (k, step) in route.iter().enumerate() {
        debug_assert_eq!(
            pos[step.stroke_idx], NO_POSITION,
            "route contains a stroke twice"
        );
        pos[step.stroke_idx] = k;
    }
 
    // Point of every node, including the origin.
    let mut points = vec![ORIGIN; 2 * stroke_count + 1];
 
    for (s, stroke) in strokes.iter().enumerate() {
        points[2 * s] = stroke.entry(false);
        points[2 * s + 1] = stroke.exit(false);
    }
 
    // Nodes that take part, and the ones that get neighbor lists.
    let mut tree: Vec<u32> = Vec::with_capacity(2 * n);
    let mut active = vec![false; points.len()];
    let mut all_finite = true;
 
    for step in route.iter() {
        for e in 0..2 {
            let node = 2 * step.stroke_idx + e;
            let p = points[node];
 
            all_finite &= p.x.is_finite() && p.y.is_finite();
            tree.push(node as u32);
            active[node] = true;
        }
    }
 
    // Non-finite coordinates make deltas meaningless (inf - inf) and can
    // keep a local search "improving" forever.
    if !all_finite {
        progress.finish();
        return;
    }
 
    // The origin gets a neighbor list but is not itself a candidate.
    active[origin_node as usize] = true;
 
    build_kd(&mut tree, &points, 0);
 
    let neighbors = build_neighbor_lists(&points, &tree, &active, |f| {
        progress.emit(f * NEIGHBOR_PHASE_WEIGHT)
    });
 
    let entries: Vec<Point> = route
        .iter()
        .map(|step| strokes[step.stroke_idx].entry(step.inverted))
        .collect();
 
    let exits: Vec<Point> = route
        .iter()
        .map(|step| strokes[step.stroke_idx].exit(step.inverted))
        .collect();
 
    let mut tour = Tour {
        route,
        entries,
        exits,
        pos,
        origin_node,
    };
 
    // Every stroke starts active, in route order for locality.
    let mut queued = vec![false; stroke_count];
    let mut queue: VecDeque<usize> = VecDeque::with_capacity(n);
 
    for step in tour.route.iter() {
        queued[step.stroke_idx] = true;
        queue.push_back(step.stroke_idx);
    }
 
    let mut processed = 0usize;
    let mut touched: Vec<usize> = Vec::with_capacity(16);
 
    while let Some(stroke) = queue.pop_front() {
        queued[stroke] = false;
        processed += 1;
 
        if let Some((delta, mv)) = tour.find_best_move(stroke, &neighbors) {
            let _ = delta;
 
            tour.touched(mv, &mut touched);
 
            #[cfg(test)]
            let length_before = tour.length();
 
            tour.apply(mv);
 
            #[cfg(test)]
            {
                let length_after = tour.length();
                assert!(
                    (length_after - (length_before + delta)).abs()
                        < 1e-6 * (1.0 + length_before),
                    "predicted delta {delta} but length changed by {}",
                    length_after - length_before
                );
            }
 
            // Re-activate everything next to a changed link, plus the
            // stroke we were working on.
            touched.push(stroke);
 
            for &t in &touched {
                if !queued[t] {
                    queued[t] = true;
                    queue.push_back(t);
                }
            }
        }
 
        if processed % 128 == 0 {
            // Work done versus work done plus work known to remain.
            let done = processed as f64;
            let fraction = done / (done + queue.len() as f64);
 
            progress.emit(
                NEIGHBOR_PHASE_WEIGHT
                    + (1.0 - NEIGHBOR_PHASE_WEIGHT) * fraction,
            );
        }
    }
 
    progress.finish();
}
 
#[cfg(test)]
mod tests {
    use super::*;
    use crate::movement::{LinearMove, Move as StrokeMove};
 
    fn make_strokes(n: usize, seed: u64) -> Vec<Stroke> {
        let mut state = seed.max(1);
 
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % 100_000) as f64 / 100.0
        };
 
        (0..n)
            .map(|_| {
                let points = vec![
                    Point::new(next(), next()),
                    Point::new(next(), next()),
                ];
 
                Stroke::new(
                    points,
                    vec![StrokeMove::Linear(LinearMove::new(None))],
                )
                .unwrap()
            })
            .collect()
    }
 
    fn route_length(strokes: &[Stroke], route: &[RouteStep]) -> f64 {
        let mut at = ORIGIN;
        let mut total = 0.0;
 
        for step in route {
            let stroke = &strokes[step.stroke_idx];
            total += at.distance(stroke.entry(step.inverted));
            at = stroke.exit(step.inverted);
        }
 
        total
    }
 
    #[test]
    fn improves_and_keeps_a_valid_permutation() {
        for (n, seed) in [
            (0, 1),
            (1, 2),
            (2, 3),
            (3, 4),
            (10, 5),
            (500, 6),
            (3000, 7),
        ] {
            let strokes = make_strokes(n, seed);
 
            let mut route: Vec<RouteStep> =
                (0..n).map(|i| RouteStep::new(i, false)).collect();
 
            let before = route_length(&strokes, &route);
            let mut last_progress = 0.0;
 
            run(&strokes, &mut route, |p| {
                assert!(p >= last_progress, "progress went backwards");
                last_progress = p;
            });
 
            assert_eq!(last_progress, 1.0);
 
            let mut seen = vec![false; n];
            for step in &route {
                assert!(!seen[step.stroke_idx]);
                seen[step.stroke_idx] = true;
            }
            assert!(seen.iter().all(|&s| s));
 
            let after = route_length(&strokes, &route);
            assert!(after <= before + 1e-9, "n={n}: {after} > {before}");
        }
    }
}
 