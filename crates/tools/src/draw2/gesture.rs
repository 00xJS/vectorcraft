//! Freehand gesture tools: Pencil (N), Paintbrush (B), Blob Brush (Shift+B), Eraser (Shift+E),
//! Knife, Smooth, Path Eraser and Join.
//!
//! Pencil and Paintbrush preview the fitted path live (`path.freehand` re-applied on the snapshot)
//! and commit on release; Alt on release (or ending near the start) closes the path, and starting
//! near an end of a selected open path continues it. The Pencil's new path starts and ends on
//! Smart Guides ([`DrawSnap`]: an anchor, a path, in line with the art), hovering too; the points
//! between follow the hand. The Paintbrush records the pen pressure of each point (a stroke a pen
//! pressed less than fully on sends `[x, y, pressure]` points) for pressure-sensitive brushes. The
//! other tools collect the drag polyline (shown as an overlay) and run
//! one command on release.

use serde_json::{Value, json};
use vectorcraft_doc::NodeId;
use vectorcraft_geom::{Point, Vec2};

use super::{FEEDBACK, points_json, polyline};
use crate::guides::DrawSnap;
use crate::{Action, Cursor, Mods, Overlay, PointerEvent, PointerKind, Tool, ToolContext, ToolKey};

pub struct GestureTool {
    id: &'static str,
    points: Vec<Point>,
    /// The pen pressure of each of `points` (the Paintbrush only).
    pressures: Vec<f32>,
    active: bool,
    began: bool,
    /// Pencil/Paintbrush continuing a selected open path: (path, continue from its start).
    extend: Option<(NodeId, bool)>,
    /// Pencil/Paintbrush starting on a selected path away from its ends: (path, Edit Selected
    /// Paths' distance in points); the stroke redraws the section it ends on (#2850).
    redraw: Option<(NodeId, f64)>,
    /// Fit tolerance in points (Pencil/Paintbrush/Smooth "Fidelity").
    pub fidelity: f64,
    /// Brush / eraser diameter in points.
    pub size: f64,
    /// Fill new pencil/brush strokes.
    pub fill: bool,
    /// "Keep selected": select the path a Pencil, Paintbrush or Blob Brush stroke draws, continues
    /// or merges into (on for the Pencil and Paintbrush, off for the Blob Brush).
    pub keep_selected: bool,
    /// "Alt key toggles to Smooth Tool": holding Alt with the Pencil tool smooths selected paths
    /// instead of closing the path (Pencil only, on by default).
    pub alt_smooth: bool,
    /// Active drag is smoothing with the Pencil tool.
    smoothing: bool,
    /// "Merge only with selection": a Blob Brush stroke merges only with selected paths.
    pub merge_only_selection: bool,
    /// "Edit selected paths within N pixels".
    pub edit_within: f64,
    /// "Close paths when ends are within N pixels".
    pub close_within: f64,
    /// Smart Guides for the Pencil's ends.
    snap: DrawSnap,
    /// Where the Pencil's stroke began, before its start snapped, and how far snapping moved it:
    /// the samples near the start take part of that move ([`ease`]).
    start_shift: (Point, Vec2),
    /// Where the pointer was for the last sample taken, before easing moved it: the steps between
    /// samples, and whether the release adds one, are measured from there.
    last_raw: Point,
    /// Where the pointer was for each of `points` (the Pencil's ends bend the stroke from these).
    raw: Vec<Point>,
}

/// How far, in multiples of the move, the samples near a snapped end follow it (#2958).
const EASE_SPAN: f64 = 4.0;

/// `p` moved by part of `shift`: all of it at `from`, none from [`EASE_SPAN`] times the shift's
/// length away, easing between. A stroke end that snaps (to the grid, a point or a guide) then
/// bends the stroke smoothly to it instead of jumping there and leaving a hook (#2958).
fn ease(p: Point, from: Point, shift: Vec2) -> Point {
    p + shift * weight(p, from, shift)
}

/// How much of `shift` a sample at `p` takes when the end at `from` moved by it ([`ease`]): 1 at
/// `from`, 0 from [`EASE_SPAN`] times the move away.
fn weight(p: Point, from: Point, shift: Vec2) -> f64 {
    let len = shift.hypot();
    if !(len > 1e-9 && len.is_finite()) {
        return 0.0;
    }
    (1.0 - (p.distance(from) / (EASE_SPAN * len)).min(1.0)).powi(2)
}

impl GestureTool {
    pub fn new(id: &str) -> Self {
        let id: &'static str = match id {
            "paintbrush" => "paintbrush",
            "blobBrush" => "blobBrush",
            "eraser" => "eraser",
            "knife" => "knife",
            "smooth" => "smooth",
            "pathEraser" => "pathEraser",
            "join" => "join",
            _ => "pencil",
        };
        let fidelity = if id == "smooth" { 2.5 } else { 1.5 };
        Self {
            id,
            points: vec![],
            pressures: vec![],
            active: false,
            began: false,
            extend: None,
            redraw: None,
            fidelity,
            size: 10.0,
            fill: false,
            keep_selected: id != "blobBrush",
            // Off, as Pencil Tool Options ships it: Alt keeps drawing a closed path until it is on.
            alt_smooth: false,
            smoothing: false,
            merge_only_selection: false,
            edit_within: 12.0,
            close_within: 15.0,
            snap: DrawSnap::default(),
            start_shift: (Point::ZERO, Vec2::ZERO),
            last_raw: Point::ZERO,
            raw: vec![],
        }
    }

    /// Do the ends of its strokes snap (the Pencil)?
    fn snaps(&self) -> bool {
        self.id == "pencil"
    }

    fn draws_path(&self) -> bool {
        matches!(self.id, "pencil" | "paintbrush")
    }

    /// Add a stroke sample.
    fn push(&mut self, p: Point, pressure: f32) {
        self.points.push(p);
        if self.id == "paintbrush" {
            self.pressures.push(pressure);
        }
    }

    /// Forget the stroke.
    fn clear(&mut self) {
        self.points.clear();
        self.pressures.clear();
        self.raw.clear();
    }

    /// The stroke's points, each `[x, y, pressure]` when a pen pressed less than fully anywhere
    /// along it (a mouse always presses fully).
    fn stroke_json(&self) -> Value {
        if self.pressures.len() == self.points.len() && self.pressures.iter().any(|p| *p < 1.0) {
            Value::Array(self.points.iter().zip(&self.pressures).map(|(p, f)| json!([p.x, p.y, (f64::from(*f) * 1000.0).round() / 1000.0])).collect())
        } else {
            points_json(&self.points)
        }
    }

    fn label(&self) -> &'static str {
        match self.id {
            "paintbrush" => "Paintbrush",
            "blobBrush" => "Blob Brush",
            "eraser" => "Eraser",
            "knife" => "Knife",
            "smooth" => "Smooth",
            "pathEraser" => "Path Eraser",
            "join" => "Join",
            _ => "Pencil",
        }
    }

    /// The selected open path whose end is near `p`.
    fn find_extend(&self, cx: &ToolContext, p: Point) -> Option<(NodeId, bool)> {
        let tol = cx.tol(self.edit_within);
        for id in &cx.selection.objects {
            let Some(pd) = cx.doc.node(*id).and_then(|n| n.path_data()) else { continue };
            if !cx.doc.is_editable(*id) {
                continue;
            }
            let Some(sp) = pd.subpaths.iter().rev().find(|s| !s.closed && !s.anchors.is_empty()) else { continue };
            let (first, last) = (sp.anchors[0].p, sp.anchors[sp.anchors.len() - 1].p);
            if p.distance(last) <= tol {
                return Some((*id, false));
            }
            if p.distance(first) <= tol {
                return Some((*id, true));
            }
        }
        None
    }

    /// The selected path whose outline is near `p` (Edit Selected Paths, off at 0 pixels).
    fn find_redraw(&self, cx: &ToolContext, p: Point) -> Option<(NodeId, f64)> {
        let tol = cx.tol(self.edit_within);
        if self.edit_within <= 0.0 || !tol.is_finite() {
            return None;
        }
        cx.selection.objects.iter().find_map(|id| {
            let pd = cx.doc.node(*id).and_then(|n| n.path_data()).filter(|_| cx.doc.is_editable(*id))?;
            pd.nearest(p).filter(|n| n.4 <= tol).map(|_| (*id, tol))
        })
    }

    /// Parameters of `path.freehand` for the current stroke.
    pub fn freehand_params(&self, closed: bool) -> Value {
        let mut v = json!({
            "points": self.stroke_json(),
            "fidelity": self.fidelity,
            "closed": closed,
            "style": if self.id == "paintbrush" { "brush" } else { "pencil" },
            "fill": self.fill,
            "keepSelected": self.keep_selected,
        });
        if let Some((id, start)) = self.extend {
            v["extend"] = json!({"id": id.0, "end": if start { "start" } else { "end" }});
        } else if let Some((id, within)) = self.redraw {
            v["redraw"] = json!({"id": id.0, "within": within});
        }
        v
    }

    fn finish(&mut self, cx: &ToolContext, m: Mods) -> Vec<Action> {
        let pts = &self.points;
        let has_sel = !cx.selection.is_empty();
        let tol = cx.tol(self.edit_within);
        if self.smoothing {
            if has_sel && pts.len() >= 2 {
                return vec![Action::Exec("path.smoothRegion".into(), json!({"points": points_json(pts), "radius": tol, "fidelity": self.fidelity}))];
            }
            return vec![];
        }
        match self.id {
            "pencil" | "paintbrush" => {
                if !self.began {
                    return vec![];
                }
                let near = pts.len() > 2 && pts[0].distance(pts[pts.len() - 1]) <= cx.tol(self.close_within);
                let closed = (m.alt && !self.alt_smooth) || (self.extend.is_none() && near);
                vec![Action::Preview("path.freehand".into(), self.freehand_params(closed)), Action::Commit]
            }
            "blobBrush" => vec![Action::Exec(
                "path.blob".into(),
                json!({
                    "points": points_json(pts),
                    "size": self.size,
                    "keepSelected": self.keep_selected,
                    "mergeOnlySelection": self.merge_only_selection
                }),
            )],
            "eraser" => vec![Action::Exec("path.eraseRegion".into(), json!({"points": points_json(pts), "size": self.size}))],
            "knife" if pts.len() >= 2 => vec![Action::Exec("path.knife".into(), json!({"points": points_json(pts)}))],
            "smooth" if has_sel && pts.len() >= 2 => {
                vec![Action::Exec("path.smoothRegion".into(), json!({"points": points_json(pts), "radius": tol, "fidelity": self.fidelity}))]
            }
            "pathEraser" if has_sel => vec![Action::Exec("path.eraseSegments".into(), json!({"points": points_json(pts), "width": cx.tol(8.0)}))],
            "join" if pts.len() >= 2 => vec![Action::Exec("path.joinScrub".into(), json!({"points": points_json(pts), "tolerance": tol}))],
            _ => vec![],
        }
    }
}

impl Tool for GestureTool {
    fn id(&self) -> &'static str {
        self.id
    }
    fn busy(&self) -> bool {
        self.active
    }
    fn pointer(&mut self, cx: &ToolContext, ev: &PointerEvent) -> Vec<Action> {
        let p = ev.pos;
        match ev.kind {
            PointerKind::Move if self.snaps() => {
                self.snap.hover(cx, p, &[], None);
                vec![]
            }
            PointerKind::Down => {
                self.active = true;
                self.began = false;
                self.smoothing = self.id == "pencil" && self.alt_smooth && ev.mods.alt;
                self.extend = if self.draws_path() && !self.smoothing { self.find_extend(cx, p) } else { None };
                self.redraw = if self.draws_path() && !self.smoothing && self.extend.is_none() { self.find_redraw(cx, p) } else { None };
                // A path continued starts at its end, one redrawn on it; a new one where it snaps.
                let start = if self.snaps() && !self.smoothing && self.extend.is_none() && self.redraw.is_none() {
                    self.snap.press(cx, p, &[], None)
                } else {
                    p
                };
                self.start_shift = (p, start - p);
                self.last_raw = p;
                self.clear();
                self.push(start, ev.pressure);
                self.raw.push(p);
                vec![]
            }
            PointerKind::Drag => {
                if !self.active {
                    return vec![];
                }
                if !self.began && self.id == "pencil" && self.alt_smooth && ev.mods.alt {
                    self.smoothing = true;
                }
                if self.snaps() && !self.smoothing {
                    // Where the stroke would end: shown, taken on release.
                    self.snap.drag(cx, p, None);
                }
                if !self.points.is_empty() && self.last_raw.distance(p) < cx.tol(1.0) {
                    return vec![];
                }
                let (from, shift) = self.start_shift;
                self.last_raw = p;
                self.raw.push(p);
                self.push(ease(p, from, shift), ev.pressure);
                if !self.draws_path() || self.smoothing {
                    return vec![];
                }
                let mut out = vec![];
                if !self.began {
                    self.began = true;
                    out.push(Action::Begin(self.label().into()));
                }
                out.push(Action::Preview("path.freehand".into(), self.freehand_params(false)));
                out
            }
            PointerKind::Up => {
                if !self.active {
                    return vec![];
                }
                let end = if self.snaps() && !self.smoothing { self.snap.drag(cx, p, None) } else { p };
                self.snap.clear();
                // Was the last step away from where the button went up (before the samples bend)?
                let away = !self.points.is_empty() && self.last_raw.distance(p) >= cx.tol(1.0);
                // The samples before a snapped end bend to it too. Near both ends (a short stroke,
                // or one whose ends snap together) a sample takes a share of each move that adds up
                // to at most the whole, so none overshoots past either end (#3013 review).
                // A smoothing pass (Alt, which may come mid-drag) works on where the hand went.
                let (end_shift, (from, start_shift)) = if self.smoothing { (Vec2::ZERO, (p, Vec2::ZERO)) } else { (end - p, self.start_shift) };
                for (q, r) in self.points.iter_mut().zip(&self.raw).skip(1) {
                    let (mut ws, mut we) = (weight(*r, from, start_shift), weight(*r, p, end_shift));
                    if ws + we > 1.0 {
                        (ws, we) = (ws / (ws + we), we / (ws + we));
                    }
                    *q = *r + start_shift * ws + end_shift * we;
                }
                if away {
                    self.push(end, ev.pressure);
                } else if self.snaps()
                    && !self.smoothing
                    && self.points.len() > 1
                    && let Some(last) = self.points.last_mut()
                {
                    // The last step was where the button went up: it moves where that snaps.
                    *last = end;
                }
                let out = self.finish(cx, ev.mods);
                self.active = false;
                self.began = false;
                self.smoothing = false;
                self.clear();
                self.extend = None;
                self.redraw = None;
                out
            }
            _ => vec![],
        }
    }
    /// `[` and `]` are the Blob Brush's and the Eraser's: their size (#1661).
    fn claims_key(&self, _cx: &ToolContext, key: ToolKey) -> bool {
        matches!(key, ToolKey::BracketLeft | ToolKey::BracketRight) && matches!(self.id, "blobBrush" | "eraser")
    }
    fn key(&mut self, _cx: &ToolContext, key: ToolKey, _m: Mods) -> Vec<Action> {
        match key {
            ToolKey::Escape if self.active => {
                let began = self.began;
                self.snap.clear();
                self.active = false;
                self.began = false;
                self.smoothing = false;
                self.clear();
                if began { vec![Action::Cancel] } else { vec![] }
            }
            ToolKey::BracketLeft if matches!(self.id, "blobBrush" | "eraser") => {
                self.size = (self.size - 1.0).max(1.0);
                vec![]
            }
            ToolKey::BracketRight if matches!(self.id, "blobBrush" | "eraser") => {
                self.size = (self.size + 1.0).min(1000.0);
                vec![]
            }
            _ => vec![],
        }
    }
    fn deactivate(&mut self, _cx: &ToolContext) -> Vec<Action> {
        let began = self.began;
        self.snap.clear();
        self.active = false;
        self.began = false;
        self.clear();
        if began { vec![Action::Commit] } else { vec![] }
    }
    fn overlays(&self, cx: &ToolContext) -> Vec<Overlay> {
        let mut o = self.snap.guides().to_vec();
        if !self.active || self.points.len() < 2 || self.began {
            return o;
        }
        let width = match self.id {
            "blobBrush" | "eraser" => (self.size * cx.zoom) as f32,
            _ => 1.0,
        };
        let color = if self.id == "eraser" { [0x80, 0x80, 0x80] } else { FEEDBACK };
        o.push(Overlay::Path { path: polyline(&self.points), color, width, dashed: self.id == "knife" });
        o
    }
    fn cursor(&self, _cx: &ToolContext, _p: Point, m: Mods) -> Cursor {
        if self.id == "pencil" && self.alt_smooth && m.alt {
            return Cursor::Smooth;
        }
        match self.id {
            "paintbrush" => Cursor::Paintbrush,
            "blobBrush" => Cursor::BlobBrush,
            "eraser" => Cursor::Eraser,
            "knife" => Cursor::Knife,
            "smooth" => Cursor::Smooth,
            "pathEraser" => Cursor::PathEraser,
            "join" => Cursor::Join,
            _ => Cursor::Pencil,
        }
    }
    fn options(&self) -> Value {
        match self.id {
            "pencil" => {
                json!({
                    "fidelity": self.fidelity,
                    "fill": self.fill,
                    "altSmooth": self.alt_smooth,
                    "keepSelected": self.keep_selected,
                    "editWithin": self.edit_within,
                    "closeWithin": self.close_within
                })
            }
            "paintbrush" => {
                json!({
                    "fidelity": self.fidelity,
                    "fill": self.fill,
                    "keepSelected": self.keep_selected,
                    "editWithin": self.edit_within,
                    "closeWithin": self.close_within
                })
            }
            "smooth" => json!({"fidelity": self.fidelity}),
            "blobBrush" => json!({"size": self.size, "keepSelected": self.keep_selected, "mergeOnlySelection": self.merge_only_selection}),
            "eraser" => json!({"size": self.size}),
            _ => Value::Null,
        }
    }
    fn set_option(&mut self, key: &str, v: &Value) {
        match key {
            "fidelity" => self.fidelity = v.as_f64().unwrap_or(self.fidelity).clamp(0.05, 100.0),
            "size" => self.size = v.as_f64().unwrap_or(self.size).clamp(0.1, 1000.0),
            "fill" => self.fill = v.as_bool().unwrap_or(self.fill),
            "altSmooth" => self.alt_smooth = v.as_bool().unwrap_or(self.alt_smooth),
            "keepSelected" => self.keep_selected = v.as_bool().unwrap_or(self.keep_selected),
            "mergeOnlySelection" => self.merge_only_selection = v.as_bool().unwrap_or(self.merge_only_selection),
            "editWithin" => self.edit_within = v.as_f64().unwrap_or(self.edit_within).clamp(0.0, 100.0),
            "closeWithin" => self.close_within = v.as_f64().unwrap_or(self.close_within).clamp(0.0, 100.0),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;
    use vectorcraft_doc::Selection;

    fn drag(t: &mut GestureTool, cx: &ToolContext, pts: &[(f64, f64)], m: Mods) -> Vec<Vec<Action>> {
        let mut v = vec![t.pointer(cx, &PointerEvent::new(PointerKind::Down, pts[0].0, pts[0].1))];
        for &(x, y) in &pts[1..] {
            v.push(t.pointer(cx, &PointerEvent::new(PointerKind::Drag, x, y)));
        }
        let l = pts[pts.len() - 1];
        v.push(t.pointer(cx, &PointerEvent::new(PointerKind::Up, l.0, l.1).with_mods(m)));
        v
    }

    fn drag_held(t: &mut GestureTool, cx: &ToolContext, pts: &[(f64, f64)], m: Mods) -> Vec<Vec<Action>> {
        let mut v = vec![t.pointer(cx, &PointerEvent::new(PointerKind::Down, pts[0].0, pts[0].1).with_mods(m))];
        for &(x, y) in &pts[1..] {
            v.push(t.pointer(cx, &PointerEvent::new(PointerKind::Drag, x, y).with_mods(m)));
        }
        let l = pts[pts.len() - 1];
        v.push(t.pointer(cx, &PointerEvent::new(PointerKind::Up, l.0, l.1).with_mods(m)));
        v
    }

    /// The Pencil's new path starts and ends on Smart Guides (#506), the points between where
    /// the hand went.
    #[test]
    fn pencil_ends_snap_to_smart_guides() {
        let (d, _) = doc_with_rect();
        let s = Selection::default();
        let p = paint();
        let cx = cx(&d, &s, &p);
        let mut t = GestureTool::new("pencil");
        let acts = drag(&mut t, &cx, &[(102.0, 98.0), (151.0, 300.0), (250.0, 320.0), (198.0, 203.0)], Mods::default());
        let Some(Action::Preview(_, v)) = acts.last().and_then(|a| a.first()) else { panic!("{acts:?}") };
        assert_eq!(v["points"], json!([[100.0, 100.0], [151.0, 300.0], [250.0, 320.0], [200.0, 200.0]]));
        // The Paintbrush follows the hand all the way.
        let mut t = GestureTool::new("paintbrush");
        let acts = drag(&mut t, &cx, &[(102.0, 98.0), (151.0, 300.0)], Mods::default());
        let Some(Action::Preview(_, v)) = acts.last().and_then(|a| a.first()) else { panic!("{acts:?}") };
        assert_eq!(v["points"][0], json!([102.0, 98.0]));
    }

    /// #2958: with Snap to Grid the Pencil's ends still land on grid points, but the stroke bends
    /// to them over a few times the distance they moved instead of jumping there: no hook (the
    /// samples keep running forward), and samples away from the ends stay where the hand went.
    #[test]
    fn pencil_strokes_ease_into_ends_snapped_to_the_grid() {
        let (d, _) = doc_with_rect();
        let s = Selection::default();
        let p = paint();
        let cx = ToolContext { snap_to_grid: true, ..cx(&d, &s, &p) };
        let step = cx.grid_step();
        // A straight stroke sampled every 2 pt, both ends between grid points.
        let (x0, x1, y) = (step * 10.0 + step * 0.45, step * 30.0 + step * 0.45, step * 10.0 + step * 0.4);
        let pts: Vec<(f64, f64)> = (0..).map(|i| x0 + 2.0 * f64::from(i)).take_while(|x| *x <= x1).map(|x| (x, y)).collect();
        let mut t = GestureTool::new("pencil");
        let acts = drag(&mut t, &cx, &pts, Mods::default());
        let Some(Action::Preview(_, v)) = acts.last().and_then(|a| a.first()) else { panic!("{acts:?}") };
        let out: Vec<(f64, f64)> = v["points"].as_array().unwrap().iter().map(|q| (q[0].as_f64().unwrap(), q[1].as_f64().unwrap())).collect();
        let on_grid = |v: f64| (v / step - (v / step).round()).abs() < 1e-9;
        let (first, last) = (out[0], out[out.len() - 1]);
        assert!(on_grid(first.0) && on_grid(first.1) && on_grid(last.0) && on_grid(last.1), "ends on the grid: {first:?} {last:?}");
        assert!(out.windows(2).all(|w| w[1].0 > w[0].0), "no hook: the stroke keeps running forward {out:?}");
        // Halfway along, the hand's points as drawn.
        let mid = out[out.len() / 2];
        assert!((mid.1 - y).abs() < 1e-9, "{mid:?}");
    }

    /// #3013 review: a stroke shorter than the start's easing has its last sample moved by it; a
    /// release where that sample was drawn still adds no step back to the hand's position.
    #[test]
    fn a_short_eased_stroke_ends_without_a_step_back() {
        let (mut d, _) = doc_with_rect();
        d.grid = vectorcraft_doc::GridPrefs { spacing: 100.0, subdivisions: 1 };
        let s = Selection::default();
        let p = paint();
        let cx = ToolContext { snap_to_grid: true, ..cx(&d, &s, &p) };
        let mut t = GestureTool::new("pencil");
        // Pressed at x 51 (snaps to 100), dragged and released at 200, on a gridline.
        let acts = drag(&mut t, &cx, &[(51.0, 100.0), (200.0, 100.0), (200.0, 100.0)], Mods::default());
        let Some(Action::Preview(_, v)) = acts.last().and_then(|a| a.first()) else { panic!("{acts:?}") };
        let xs: Vec<f64> = v["points"].as_array().unwrap().iter().map(|q| q[0].as_f64().unwrap()).collect();
        assert!(xs.windows(2).all(|w| w[1] > w[0]), "no step back: {xs:?}");
        assert_eq!((xs.first().copied(), xs.last().copied()), (Some(100.0), Some(200.0)), "{xs:?}");
    }

    /// #3013 review: a short stroke whose start and end snap to the same grid point keeps every
    /// sample, and none is moved by both snaps at once: each moves by at most the larger of them.
    #[test]
    fn a_stroke_whose_ends_snap_together_keeps_its_samples_without_overshoot() {
        let (mut d, _) = doc_with_rect();
        d.grid = vectorcraft_doc::GridPrefs { spacing: 100.0, subdivisions: 1 };
        let s = Selection::default();
        let p = paint();
        let cx = ToolContext { snap_to_grid: true, ..cx(&d, &s, &p) };
        // A small loop around (106, 106): both ends snap to the grid point (100, 100).
        let raw: Vec<(f64, f64)> =
            (0..=24).map(|i| f64::from(i) * std::f64::consts::TAU / 24.0).map(|a| (106.0 + 4.0 * a.cos(), 106.0 + 4.0 * a.sin())).collect();
        let mut t = GestureTool::new("pencil");
        let acts = drag(&mut t, &cx, &raw, Mods::default());
        let Some(Action::Preview(_, v)) = acts.last().and_then(|a| a.first()) else { panic!("{acts:?}") };
        let out: Vec<(f64, f64)> = v["points"].as_array().unwrap().iter().map(|q| (q[0].as_f64().unwrap(), q[1].as_f64().unwrap())).collect();
        assert!(out.len() >= raw.len() - 1, "samples kept: {} of {}", out.len(), raw.len());
        assert_eq!(out[0], (100.0, 100.0));
        // Both ends moved from the same place to (100, 100): no sample moves further than that.
        let most = (raw[0].0 - 100.0).hypot(raw[0].1 - 100.0) + 1e-9;
        for (q, r) in out.iter().zip(&raw).skip(1) {
            assert!((q.0 - r.0).hypot(q.1 - r.1) <= most, "{q:?} from {r:?}");
        }
    }

    /// #852: the Paintbrush sends each point's pen pressure; a mouse (always fully pressed) and
    /// the Pencil send plain points.
    #[test]
    fn paintbrush_sends_pen_pressure() {
        let (d, _) = doc_with_rect();
        let s = Selection::default();
        let p = paint();
        let cx = cx(&d, &s, &p);
        let stroke = |id: &str, pressures: [f32; 3]| {
            let mut t = GestureTool::new(id);
            let kinds = [PointerKind::Down, PointerKind::Drag, PointerKind::Up];
            let mut last = vec![];
            for ((k, x), f) in kinds.into_iter().zip([0.0, 40.0, 80.0]).zip(pressures) {
                last = t.pointer(&cx, &PointerEvent { pressure: f, ..PointerEvent::new(k, x, 0.0) });
            }
            let Some(Action::Preview(_, v)) = last.first() else { panic!("{last:?}") };
            v["points"].clone()
        };
        assert_eq!(stroke("paintbrush", [0.25, 0.5, 1.0]), json!([[0.0, 0.0, 0.25], [40.0, 0.0, 0.5], [80.0, 0.0, 1.0]]));
        assert_eq!(stroke("paintbrush", [1.0; 3]), json!([[0.0, 0.0], [40.0, 0.0], [80.0, 0.0]]));
        assert_eq!(stroke("pencil", [0.25, 0.5, 1.0]), json!([[0.0, 0.0], [40.0, 0.0], [80.0, 0.0]]));
    }

    #[test]
    fn pencil_previews_and_commits() {
        let (d, _) = doc_with_rect();
        let s = Selection::default();
        let p = paint();
        let cx = cx(&d, &s, &p);
        let mut t = GestureTool::new("pencil");
        let acts = drag(&mut t, &cx, &[(0.0, 0.0), (10.0, 5.0), (20.0, 0.0)], Mods::default());
        assert!(acts[0].is_empty());
        assert_eq!(acts[1][0], Action::Begin("Pencil".into()));
        assert!(matches!(&acts[1][1], Action::Preview(c, v) if c == "path.freehand" && v["points"].as_array().unwrap().len() == 2));
        let last = acts.last().unwrap();
        assert!(matches!(&last[0], Action::Preview(_, v) if v["closed"] == false && v["style"] == "pencil"));
        assert_eq!(last[1], Action::Commit);
        assert!(!t.busy());
    }

    /// Keep selected is on by default, and every Pencil and Paintbrush preview sends it.
    #[test]
    fn pencil_and_paintbrush_send_keep_selected() {
        let (d, _) = doc_with_rect();
        let s = Selection::default();
        let p = paint();
        let cx = cx(&d, &s, &p);
        for id in ["pencil", "paintbrush"] {
            let mut t = GestureTool::new(id);
            assert_eq!(t.options()["keepSelected"], json!(true), "{id}");
            for keep in [true, false] {
                t.set_option("keepSelected", &json!(keep));
                assert_eq!(t.options()["keepSelected"], json!(keep), "{id}");
                let acts = drag(&mut t, &cx, &[(0.0, 0.0), (10.0, 5.0), (20.0, 0.0)], Mods::default());
                assert!(acts[1..].iter().all(|a| a.iter().all(|a| !matches!(a, Action::Preview(_, v) if v["keepSelected"] != keep))), "{id}");
                assert!(matches!(&acts.last().unwrap()[0], Action::Preview(_, v) if v["keepSelected"] == keep), "{id}");
            }
        }
    }

    /// The Blob Brush has Keep selected and Merge only with selection, both off by default, and its
    /// stroke sends both.
    #[test]
    fn blob_brush_sends_keep_selected_and_merge_only_with_selection() {
        let (d, _) = doc_with_rect();
        let s = Selection::default();
        let p = paint();
        let cx = cx(&d, &s, &p);
        let mut t = GestureTool::new("blobBrush");
        assert_eq!((t.options()["keepSelected"].clone(), t.options()["mergeOnlySelection"].clone()), (json!(false), json!(false)));
        for (keep, only) in [(false, false), (true, false), (false, true)] {
            t.set_option("keepSelected", &json!(keep));
            t.set_option("mergeOnlySelection", &json!(only));
            let acts = drag(&mut t, &cx, &[(50.0, 150.0), (250.0, 150.0)], Mods::default());
            assert!(
                matches!(&acts[2][0], Action::Exec(c, v) if c == "path.blob" && v["keepSelected"] == keep && v["mergeOnlySelection"] == only),
                "{acts:?}"
            );
        }
        // The Eraser keeps Size only.
        assert_eq!(GestureTool::new("eraser").options(), json!({"size": 10.0}));
    }

    #[test]
    fn pencil_alt_closes_and_continues_selected_path() {
        let (d, id) = doc_with_rect();
        let s = Selection::default();
        let p = paint();
        let cx = cx(&d, &s, &p);
        let mut t = GestureTool::new("paintbrush");
        let acts = drag(&mut t, &cx, &[(0.0, 0.0), (10.0, 5.0), (20.0, 0.0)], Mods { alt: true, ..Default::default() });
        assert!(matches!(&acts.last().unwrap()[0], Action::Preview(_, v) if v["closed"] == true && v["style"] == "brush"));
        // A selected closed rect is not continued; an open path would be (see engine tests).
        let mut s2 = Selection::default();
        s2.set([id]);
        let cx2 = crate::testutil::cx(&d, &s2, &p);
        let acts = drag(&mut t, &cx2, &[(100.0, 100.0), (90.0, 90.0)], Mods::default());
        assert!(matches!(&acts[1][1], Action::Preview(_, v) if v.get("extend").is_none()));
    }

    #[test]
    fn eraser_and_knife_exec_on_release() {
        let (d, _) = doc_with_rect();
        let s = Selection::default();
        let p = paint();
        let cx = cx(&d, &s, &p);
        let mut t = GestureTool::new("eraser");
        let acts = drag(&mut t, &cx, &[(50.0, 150.0), (250.0, 150.0)], Mods::default());
        assert!(acts[1].is_empty());
        assert!(matches!(&acts[2][0], Action::Exec(c, v) if c == "path.eraseRegion" && v["size"] == 10.0));
        let mut k = GestureTool::new("knife");
        k.pointer(&cx, &PointerEvent::new(PointerKind::Down, 50.0, 150.0));
        k.pointer(&cx, &PointerEvent::new(PointerKind::Drag, 150.0, 150.0));
        assert_eq!(k.overlays(&cx).len(), 1);
        let a = k.pointer(&cx, &PointerEvent::new(PointerKind::Up, 250.0, 150.0));
        assert!(matches!(&a[0], Action::Exec(c, v) if c == "path.knife" && v["points"].as_array().unwrap().len() == 3));
    }

    #[test]
    fn smooth_needs_selection_and_brackets_resize() {
        let (d, _) = doc_with_rect();
        let s = Selection::default();
        let p = paint();
        let cx = cx(&d, &s, &p);
        let mut t = GestureTool::new("smooth");
        let acts = drag(&mut t, &cx, &[(50.0, 150.0), (250.0, 150.0)], Mods::default());
        assert!(acts.iter().all(|a| a.is_empty()));
        let mut b = GestureTool::new("blobBrush");
        b.key(&cx, ToolKey::BracketRight, Mods::default());
        assert_eq!(b.options()["size"], 11.0);
    }

    #[test]
    fn pencil_alt_toggles_to_smooth_when_enabled_and_closes_when_disabled() {
        let (d, id) = doc_with_rect();
        let mut s = Selection::default();
        s.set([id]);
        let p = paint();
        let cx = cx(&d, &s, &p);

        // Off by default, as Pencil Tool Options ships it; on, Alt lends the Smooth tool.
        let mut t = GestureTool::new("pencil");
        assert_eq!(t.options()["altSmooth"], json!(false));
        t.set_option("altSmooth", &json!(true));

        // When alt is pressed, cursor shows smooth tool.
        let alt_mod = Mods { alt: true, ..Default::default() };
        assert_eq!(t.cursor(&cx, Point::new(50.0, 50.0), alt_mod), Cursor::Smooth);
        assert_eq!(t.cursor(&cx, Point::new(50.0, 50.0), Mods::default()), Cursor::Pencil);

        // Alt-drag over selected path smooths it.
        let acts = drag_held(&mut t, &cx, &[(50.0, 150.0), (150.0, 150.0), (250.0, 150.0)], alt_mod);
        assert!(acts[1].is_empty(), "drag does not preview freehand path while smoothing");
        let last = acts.last().unwrap();
        assert!(
            matches!(&last[0], Action::Exec(c, v) if c == "path.smoothRegion" && v["points"].as_array().unwrap().len() == 3),
            "expected path.smoothRegion, got: {last:?}"
        );

        // When altSmooth is disabled:
        t.set_option("altSmooth", &json!(false));
        assert_eq!(t.options()["altSmooth"], json!(false));
        assert_eq!(t.cursor(&cx, Point::new(50.0, 50.0), alt_mod), Cursor::Pencil);

        // Alt-drag draws a closed pencil path.
        let empty_sel = Selection::default();
        let cx_empty = crate::testutil::cx(&d, &empty_sel, &p);
        let acts = drag(&mut t, &cx_empty, &[(0.0, 0.0), (10.0, 5.0), (20.0, 0.0)], alt_mod);
        assert_eq!(acts[1][0], Action::Begin("Pencil".into()));
        let last = acts.last().unwrap();
        assert!(matches!(&last[0], Action::Preview(_, v) if v["closed"] == true && v["style"] == "pencil"));
        assert_eq!(last[1], Action::Commit);
    }
}
