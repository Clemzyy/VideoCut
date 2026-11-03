//! Vue de la forme d'onde : zoom à la molette, marqueurs Début/Fin et tête
//! de lecture. Le tracé lit la pyramide de pics, donc un coût proportionnel
//! à la largeur de l'écran quel que soit le zoom.

use crate::media::Peaks;
use crate::timefmt::format_time;
use egui::{
    Align2, Color32, CursorIcon, Event, FontFamily, FontId, Mesh, MouseWheelUnit, Painter, Rect, Response, Sense,
    Shape, Stroke, StrokeKind, Ui, pos2,
};

/// Écart minimal entre Début et Fin, en secondes.
pub const MIN_SELECTION: f64 = 0.02;
/// Plage visible minimale au zoom maximal, en secondes.
pub const MIN_SPAN: f64 = 0.05;
const GRAB_DISTANCE: f32 = 18.0;
const ZOOM_IN_STEP: f64 = 0.82;

const BG: Color32 = Color32::from_rgb(0x10, 0x14, 0x18);
const BORDER: Color32 = Color32::from_rgb(0x2c, 0x35, 0x40);
const MID_LINE: Color32 = Color32::from_rgb(0x33, 0x41, 0x4d);
const WAVE: Color32 = Color32::from_rgb(0x57, 0xc7, 0xff);
const DIM: Color32 = Color32::from_rgba_premultiplied(3, 4, 5, 150);
const SELECTION_OUTLINE: Color32 = Color32::from_rgb(0xdc, 0xec, 0xff);
const MARKER_SHADOW: Color32 = Color32::from_rgb(0x0b, 0x10, 0x14);
pub const START_COLOR: Color32 = Color32::from_rgb(0x2e, 0xe5, 0x9d);
pub const END_COLOR: Color32 = Color32::from_rgb(0xff, 0xb4, 0x54);
const LABEL: Color32 = Color32::from_rgb(0xc9, 0xd4, 0xdd);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Start,
    End,
    Playhead,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WaveEvent {
    DragStarted(Marker),
    Moved(Marker, f64),
    DragFinished(Marker),
    /// Choix fait dans le menu du clic droit.
    Menu(MenuAction),
}

/// Actions du menu contextuel ; l'heure est celle de l'endroit cliqué.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MenuAction {
    SetStart(f64),
    SetEnd(f64),
    PlayFrom(f64),
    SelectAll,
}

#[derive(Debug, Clone)]
pub struct WaveformView {
    duration: f64,
    pub start: f64,
    pub end: f64,
    pub playhead: f64,
    view_start: f64,
    view_end: f64,
    dragging: Option<Marker>,
    /// Heure sous la souris au moment du clic droit.
    menu_time: f64,
}

impl Default for WaveformView {
    fn default() -> Self {
        Self::new(1.0)
    }
}

impl WaveformView {
    pub fn new(duration: f64) -> Self {
        let duration = if duration.is_finite() { duration.max(0.001) } else { 0.001 };
        Self {
            duration,
            start: 0.0,
            end: duration,
            playhead: 0.0,
            view_start: 0.0,
            view_end: duration,
            dragging: None,
            menu_time: 0.0,
        }
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn view_range(&self) -> (f64, f64) {
        (self.view_start, self.view_end)
    }

    pub fn dragging(&self) -> Option<Marker> {
        self.dragging
    }

    pub fn set_start(&mut self, seconds: f64) {
        let max = (self.end - MIN_SELECTION).max(0.0);
        self.start = seconds.clamp(0.0, max);
    }

    pub fn set_end(&mut self, seconds: f64) {
        let min = (self.start + MIN_SELECTION).min(self.duration);
        self.end = seconds.clamp(min, self.duration);
    }

    pub fn set_playhead(&mut self, seconds: f64) {
        self.playhead = seconds.clamp(0.0, self.duration);
    }

    /// Place la tête et fait défiler la vue si elle en sort.
    pub fn follow_playhead(&mut self, seconds: f64) {
        self.set_playhead(seconds);
        if self.playhead < self.view_start || self.playhead > self.view_end {
            let span = self.view_end - self.view_start;
            self.view_start = (self.playhead - span * 0.25).clamp(0.0, self.duration - span);
            self.view_end = self.view_start + span;
        }
    }

    /// Zoome autour de `anchor` : le point sous la souris reste immobile.
    pub fn zoom(&mut self, anchor: f64, factor: f64) {
        let span = self.view_end - self.view_start;
        let new_span = (span * factor).clamp(MIN_SPAN.min(self.duration), self.duration);
        let ratio = ((anchor - self.view_start) / span.max(1e-9)).clamp(0.0, 1.0);
        let new_start = (anchor - ratio * new_span).clamp(0.0, self.duration - new_span);
        self.view_start = new_start;
        self.view_end = new_start + new_span;
    }

    pub fn pan(&mut self, delta_seconds: f64) {
        let span = self.view_end - self.view_start;
        self.view_start = (self.view_start + delta_seconds).clamp(0.0, self.duration - span);
        self.view_end = self.view_start + span;
    }

    /// Affiche `[from, to]` avec une petite marge de chaque côté.
    pub fn zoom_to(&mut self, from: f64, to: f64) {
        let margin = (to - from) * 0.05;
        let span = (to - from + 2.0 * margin).clamp(MIN_SPAN.min(self.duration), self.duration);
        let center = (from + to) / 2.0;
        self.view_start = (center - span / 2.0).clamp(0.0, self.duration - span);
        self.view_end = self.view_start + span;
    }

    pub fn reset_zoom(&mut self) {
        self.view_start = 0.0;
        self.view_end = self.duration;
    }

    fn time_to_x(&self, seconds: f64, rect: Rect) -> f32 {
        let span = (self.view_end - self.view_start).max(1e-9);
        rect.left() + ((seconds - self.view_start) / span) as f32 * rect.width()
    }

    fn x_to_time(&self, x: f32, rect: Rect) -> f64 {
        let ratio = f64::from(((x - rect.left()) / rect.width().max(1.0)).clamp(0.0, 1.0));
        self.view_start + ratio * (self.view_end - self.view_start)
    }

    /// Marqueur saisi par un clic : le plus proche à moins de 18 px, sinon la tête.
    fn pick(&self, x: f32, rect: Rect) -> Marker {
        [Marker::Start, Marker::End, Marker::Playhead]
            .into_iter()
            .map(|marker| (marker, (self.time_to_x(self.marker_time(marker), rect) - x).abs()))
            .filter(|(_, distance)| *distance <= GRAB_DISTANCE)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map_or(Marker::Playhead, |(marker, _)| marker)
    }

    fn marker_time(&self, marker: Marker) -> f64 {
        match marker {
            Marker::Start => self.start,
            Marker::End => self.end,
            Marker::Playhead => self.playhead,
        }
    }

    /// Déplace un marqueur et renvoie sa nouvelle position.
    fn drag_to(&mut self, marker: Marker, seconds: f64) -> f64 {
        let seconds = seconds.clamp(0.0, self.duration);
        match marker {
            Marker::Start => {
                self.set_start(seconds);
                self.playhead = self.start;
                self.start
            }
            Marker::End => {
                self.set_end(seconds);
                self.playhead = self.end;
                self.end
            }
            Marker::Playhead => {
                self.playhead = seconds.clamp(self.start, self.end);
                self.playhead
            }
        }
    }

    /// Dessine la vue et traite la souris. Renvoie les déplacements effectués.
    pub fn show(&mut self, ui: &mut Ui, peaks: Option<&Peaks>, placeholder: Option<&str>) -> Vec<WaveEvent> {
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let mut events = Vec::new();
        if placeholder.is_none() {
            self.handle_input(ui, rect, &response, &mut events);
            self.context_menu(&response, &mut events);
        }
        let painter = ui.painter_at(rect);
        self.paint(&painter, rect, peaks, placeholder);
        events
    }

    /// Menu du clic droit (un clic droit glissé sert toujours à se déplacer).
    fn context_menu(&mut self, response: &Response, events: &mut Vec<WaveEvent>) {
        let time = self.menu_time;
        let (start, end) = (self.start, self.end);
        response.context_menu(|ui| {
            ui.label(egui::RichText::new(format_time(time)).monospace().weak());
            ui.separator();
            if ui.button("Placer le début ici\tI").clicked() {
                events.push(WaveEvent::Menu(MenuAction::SetStart(time)));
            }
            if ui.button("Placer la fin ici\tO").clicked() {
                events.push(WaveEvent::Menu(MenuAction::SetEnd(time)));
            }
            if ui.button("Lire depuis ici").clicked() {
                events.push(WaveEvent::Menu(MenuAction::PlayFrom(time)));
            }
            ui.separator();
            if ui.button("Zoomer sur la sélection").clicked() {
                self.zoom_to(start, end);
            }
            if ui.button("Tout afficher\tdouble-clic").clicked() {
                self.reset_zoom();
            }
            if ui.button("Sélectionner tout le fichier").clicked() {
                events.push(WaveEvent::Menu(MenuAction::SelectAll));
            }
        });
    }

    fn handle_input(&mut self, ui: &Ui, rect: Rect, response: &Response, events: &mut Vec<WaveEvent>) {
        if response.hovered() && ui.input(|i| i.pointer.secondary_pressed()) {
            if let Some(pos) = response.interact_pointer_pos().or(response.hover_pos()) {
                self.menu_time = self.x_to_time(pos.x, rect);
            }
        }
        let (pressed, down, pos, delta, panning, shift, wheel) = ui.input(|i| {
            let wheel: Vec<(MouseWheelUnit, egui::Vec2)> = i
                .events
                .iter()
                .filter_map(|event| match event {
                    Event::MouseWheel { unit, delta, .. } => Some((*unit, *delta)),
                    _ => None,
                })
                .collect();
            (
                i.pointer.primary_pressed(),
                i.pointer.primary_down(),
                i.pointer.interact_pos(),
                i.pointer.delta(),
                i.pointer.secondary_down() || i.pointer.middle_down(),
                i.modifiers.shift,
                wheel,
            )
        });

        if let Some(marker) = self.dragging {
            match pos {
                Some(pos) if down => {
                    if delta != egui::Vec2::ZERO {
                        let value = self.drag_to(marker, self.x_to_time(pos.x, rect));
                        events.push(WaveEvent::Moved(marker, value));
                    }
                }
                _ => {
                    self.dragging = None;
                    events.push(WaveEvent::DragFinished(marker));
                }
            }
        } else if pressed && response.contains_pointer() {
            if let Some(pos) = pos {
                let marker = self.pick(pos.x, rect);
                self.dragging = Some(marker);
                events.push(WaveEvent::DragStarted(marker));
                let value = self.drag_to(marker, self.x_to_time(pos.x, rect));
                events.push(WaveEvent::Moved(marker, value));
            }
        }

        if response.double_clicked() {
            self.reset_zoom();
        }

        if response.hovered() {
            let span = self.view_end - self.view_start;
            if panning && delta.x != 0.0 {
                self.pan(-f64::from(delta.x / rect.width().max(1.0)) * span);
            }
            for (unit, delta) in wheel {
                let scale = match unit {
                    MouseWheelUnit::Point => 1.0 / 50.0,
                    MouseWheelUnit::Line => 1.0,
                    MouseWheelUnit::Page => 10.0,
                };
                let (dx, dy) = (f64::from(delta.x) * scale, f64::from(delta.y) * scale);
                let span = self.view_end - self.view_start;
                if shift || dx.abs() > dy.abs() {
                    let amount = if shift { dx + dy } else { dx };
                    self.pan(-amount * span * 0.1);
                } else if dy != 0.0 {
                    let anchor = pos.map_or((self.view_start + self.view_end) / 2.0, |p| self.x_to_time(p.x, rect));
                    self.zoom(anchor, ZOOM_IN_STEP.powf(dy));
                }
            }
            let near_marker = pos.is_some_and(|p| self.pick(p.x, rect) != Marker::Playhead);
            if self.dragging.is_some() || near_marker {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            } else if panning {
                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
            }
        }
    }

    fn paint(&self, painter: &Painter, rect: Rect, peaks: Option<&Peaks>, placeholder: Option<&str>) {
        painter.rect_filled(rect, 0, BG);
        let mid = rect.center().y;
        painter.hline(rect.x_range(), mid, Stroke::new(1.0, MID_LINE));
        if let Some(text) = placeholder {
            painter.text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(15.0), LABEL);
            painter.rect_stroke(rect, 0, Stroke::new(1.0, BORDER), StrokeKind::Inside);
            return;
        }

        if let Some(peaks) = peaks {
            self.paint_wave(painter, rect, peaks);
        }

        let start_x = self.time_to_x(self.start, rect);
        let end_x = self.time_to_x(self.end, rect);
        if start_x > rect.left() {
            let dim = Rect::from_min_max(rect.min, pos2(start_x.min(rect.right()), rect.bottom()));
            painter.rect_filled(dim, 0, DIM);
        }
        if end_x < rect.right() {
            let dim = Rect::from_min_max(pos2(end_x.max(rect.left()), rect.top()), rect.max);
            painter.rect_filled(dim, 0, DIM);
        }
        if start_x < end_x {
            let selection = Rect::from_x_y_ranges(start_x..=end_x, rect.y_range()).intersect(rect);
            painter.rect_stroke(selection, 0, Stroke::new(1.0, SELECTION_OUTLINE), StrokeKind::Inside);
        }

        self.paint_marker(painter, rect, self.start, START_COLOR, "DÉBUT");
        self.paint_marker(painter, rect, self.end, END_COLOR, "FIN");
        let play_x = self.time_to_x(self.playhead, rect);
        if (rect.left()..=rect.right()).contains(&play_x) {
            painter.vline(play_x, rect.y_range(), Stroke::new(2.0, Color32::WHITE));
        }

        let label = format!("{}  –  {}", format_time(self.view_start), format_time(self.view_end));
        painter.text(
            pos2(rect.right() - 10.0, rect.top() + 14.0),
            Align2::RIGHT_CENTER,
            label,
            FontId::proportional(12.0),
            LABEL,
        );
        painter.rect_stroke(rect, 0, Stroke::new(1.0, BORDER), StrokeKind::Inside);
    }

    /// Une colonne par pixel physique, chacune lue dans la pyramide.
    fn paint_wave(&self, painter: &Painter, rect: Rect, peaks: &Peaks) {
        let pixels_per_point = painter.ctx().pixels_per_point();
        let columns = (rect.width() * pixels_per_point).round().max(1.0) as usize;
        let step = rect.width() / columns as f32;
        let span = self.view_end - self.view_start;
        let amplitude = rect.height() * 0.46;
        let mid = rect.center().y;
        let mut mesh = Mesh::default();
        for column in 0..columns {
            let t0 = self.view_start + span * column as f64 / columns as f64;
            let t1 = self.view_start + span * (column + 1) as f64 / columns as f64;
            let Some((min, max)) = peaks.range(t0, t1) else { continue };
            let mut top = mid - max * amplitude;
            let mut bottom = mid - min * amplitude;
            if bottom - top < 1.0 {
                top -= 0.5;
                bottom += 0.5;
            }
            let x = rect.left() + column as f32 * step;
            mesh.add_colored_rect(Rect::from_min_max(pos2(x, top), pos2(x + step, bottom)), WAVE);
        }
        painter.add(Shape::mesh(mesh));
    }

    fn paint_marker(&self, painter: &Painter, rect: Rect, seconds: f64, color: Color32, label: &str) {
        let x = self.time_to_x(seconds, rect);
        if x < rect.left() - 20.0 || x > rect.right() + 20.0 {
            return;
        }
        let (top, bottom) = (rect.top(), rect.bottom());
        painter.vline(x - 1.0, rect.y_range(), Stroke::new(7.0, MARKER_SHADOW));
        painter.vline(x, rect.y_range(), Stroke::new(5.0, color));
        let outline = Stroke::new(1.0, Color32::WHITE);
        painter.add(Shape::convex_polygon(
            vec![pos2(x - 12.0, top), pos2(x + 12.0, top), pos2(x, top + 18.0)],
            color,
            outline,
        ));
        painter.add(Shape::convex_polygon(
            vec![pos2(x - 12.0, bottom), pos2(x, bottom - 18.0), pos2(x + 12.0, bottom)],
            color,
            outline,
        ));
        let flip = x + 62.0 > rect.right() - 4.0;
        let (left, right) = if flip { (x - 62.0, x - 8.0) } else { (x + 8.0, x + 62.0) };
        let label_rect = Rect::from_min_max(pos2(left, top + 22.0), pos2(right, top + 44.0));
        painter.rect_filled(label_rect, 3, MARKER_SHADOW);
        painter.rect_stroke(label_rect, 3, Stroke::new(1.0, color), StrokeKind::Inside);
        painter.text(
            label_rect.center(),
            Align2::CENTER_CENTER,
            label,
            FontId::new(12.0, FontFamily::Name("bold".into())),
            color,
        );
    }
}

/// Plus grand rectangle centré dans `container` qui respecte le ratio d'image.
pub fn fit_rect(container: Rect, width: f32, height: f32) -> Rect {
    if width <= 0.0 || height <= 0.0 {
        return container;
    }
    let scale = (container.width() / width).min(container.height() / height);
    Rect::from_center_size(container.center(), egui::vec2(width * scale, height * scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect() -> Rect {
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 200.0))
    }

    #[test]
    fn zoom_keeps_the_anchor_under_the_cursor() {
        let mut view = WaveformView::new(100.0);
        view.zoom(25.0, 0.5);
        let (start, end) = view.view_range();
        assert!((end - start - 50.0).abs() < 1e-9);
        // 25 s était à 25 % de la largeur : il doit y rester.
        assert!(((25.0 - start) / (end - start) - 0.25).abs() < 1e-9);
    }

    #[test]
    fn zoom_is_clamped_to_limits() {
        let mut view = WaveformView::new(100.0);
        for _ in 0..200 {
            view.zoom(99.99, ZOOM_IN_STEP);
        }
        let (start, end) = view.view_range();
        assert!((end - start - MIN_SPAN).abs() < 1e-9);
        assert!(end <= 100.0 + 1e-9);
        for _ in 0..200 {
            view.zoom(50.0, 1.0 / ZOOM_IN_STEP);
        }
        assert_eq!(view.view_range(), (0.0, 100.0));
    }

    #[test]
    fn zoom_to_frames_the_selection_with_a_margin() {
        let mut view = WaveformView::new(100.0);
        view.zoom_to(40.0, 60.0);
        assert_eq!(view.view_range(), (39.0, 61.0));
        // Près des bords, la vue reste dans le fichier.
        view.zoom_to(0.0, 10.0);
        assert_eq!(view.view_range(), (0.0, 11.0));
        // Sélection minuscule : on s'arrête au zoom maximal.
        view.zoom_to(50.0, 50.001);
        assert!((view.view_range().1 - view.view_range().0 - MIN_SPAN).abs() < 1e-9);
    }

    #[test]
    fn pan_stays_inside_the_media() {
        let mut view = WaveformView::new(100.0);
        view.zoom(50.0, 0.1);
        view.pan(-1000.0);
        assert_eq!(view.view_range().0, 0.0);
        view.pan(1000.0);
        assert!((view.view_range().1 - 100.0).abs() < 1e-9);
    }

    #[test]
    fn selection_keeps_a_minimum_gap() {
        let mut view = WaveformView::new(10.0);
        view.set_end(4.0);
        view.set_start(9.0);
        assert!((view.start - (4.0 - MIN_SELECTION)).abs() < 1e-9);
        view.set_end(-3.0);
        assert!((view.end - (view.start + MIN_SELECTION)).abs() < 1e-9);
        view.set_end(50.0);
        assert_eq!(view.end, 10.0);
    }

    #[test]
    fn clicking_picks_the_nearest_marker_or_the_playhead() {
        let mut view = WaveformView::new(100.0);
        view.set_start(10.0);
        view.set_end(90.0);
        view.set_playhead(50.0);
        assert_eq!(view.pick(105.0, rect()), Marker::Start);
        assert_eq!(view.pick(890.0, rect()), Marker::End);
        assert_eq!(view.pick(300.0, rect()), Marker::Playhead);
    }

    #[test]
    fn dragging_markers_moves_the_playhead_like_the_original() {
        let mut view = WaveformView::new(100.0);
        assert_eq!(view.drag_to(Marker::Start, 20.0), 20.0);
        assert_eq!(view.playhead, 20.0);
        assert_eq!(view.drag_to(Marker::End, 80.0), 80.0);
        assert_eq!(view.playhead, 80.0);
        // La tête reste dans la sélection, comme dans la version Python.
        assert_eq!(view.drag_to(Marker::Playhead, 95.0), 80.0);
        assert_eq!(view.drag_to(Marker::Playhead, 5.0), 20.0);
    }

    #[test]
    fn follow_scrolls_the_view_to_the_playhead() {
        let mut view = WaveformView::new(100.0);
        view.zoom(0.0, 0.1);
        view.follow_playhead(60.0);
        let (start, end) = view.view_range();
        assert!(start <= 60.0 && 60.0 <= end);
        assert!((end - start - 10.0).abs() < 1e-9);
    }

    #[test]
    fn coordinates_round_trip() {
        let mut view = WaveformView::new(200.0);
        view.zoom(100.0, 0.25);
        for t in [80.0, 100.0, 119.0] {
            let x = view.time_to_x(t, rect());
            assert!((view.x_to_time(x, rect()) - t).abs() < 1e-3);
        }
    }

    #[test]
    fn fit_rect_preserves_aspect_ratio() {
        let fitted = fit_rect(Rect::from_min_max(pos2(0.0, 0.0), pos2(400.0, 400.0)), 1920.0, 1080.0);
        assert!((fitted.width() - 400.0).abs() < 1e-3);
        assert!((fitted.height() - 225.0).abs() < 1e-3);
        assert!((fitted.center().y - 200.0).abs() < 1e-3);
    }
}
