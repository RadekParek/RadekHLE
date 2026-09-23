/*
 * This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/.
 */

use super::*;
use crate::gles::gles11_raw::types::{GLenum, GLfloat, GLsizei, GLvoid};
use crate::trainer::classify::Category;

static INPUT_TEST_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, PartialEq)]
struct Draw {
    texture: Option<GLuint>,
    color: (f32, f32, f32, f32),
}

/// Models the state left by present_frame, without needing a window or GPU.
struct RecordingGles {
    textured: bool,
    texture: GLuint,
    tex_env_mode: GLint,
    color: (f32, f32, f32, f32),
    draws: Vec<Draw>,
}

#[allow(non_snake_case)]
impl GLES for RecordingGles {
    unsafe fn GetIntegerv(&mut self, pname: GLenum, params: *mut GLint) {
        *params = match pname {
            gles11::ACTIVE_TEXTURE => gles11::TEXTURE0 as GLint,
            gles11::TEXTURE_BINDING_2D => self.texture as GLint,
            _ => panic!("unexpected state query: {pname:#x}"),
        };
    }

    unsafe fn GetTexEnviv(&mut self, target: GLenum, pname: GLenum, params: *mut GLint) {
        assert_eq!(target, gles11::TEXTURE_ENV);
        assert_eq!(pname, gles11::TEXTURE_ENV_MODE);
        *params = self.tex_env_mode;
    }

    unsafe fn TexEnviv(&mut self, target: GLenum, pname: GLenum, params: *const GLint) {
        assert_eq!(target, gles11::TEXTURE_ENV);
        assert_eq!(pname, gles11::TEXTURE_ENV_MODE);
        self.tex_env_mode = *params;
    }

    unsafe fn ActiveTexture(&mut self, texture: GLenum) {
        assert_eq!(texture, gles11::TEXTURE0);
    }

    unsafe fn BindTexture(&mut self, target: GLenum, texture: GLuint) {
        assert_eq!(target, gles11::TEXTURE_2D);
        self.texture = texture;
    }

    unsafe fn Enable(&mut self, cap: GLenum) {
        match cap {
            gles11::TEXTURE_2D => self.textured = true,
            gles11::BLEND => (),
            _ => panic!("unexpected capability: {cap:#x}"),
        }
    }

    unsafe fn Disable(&mut self, cap: GLenum) {
        match cap {
            gles11::TEXTURE_2D => self.textured = false,
            gles11::BLEND => (),
            _ => panic!("unexpected capability: {cap:#x}"),
        }
    }

    unsafe fn Color4f(&mut self, r: GLfloat, g: GLfloat, b: GLfloat, a: GLfloat) {
        self.color = (r, g, b, a);
    }

    unsafe fn DrawArrays(&mut self, mode: GLenum, first: GLint, count: GLsizei) {
        assert_eq!((mode, first, count), (gles11::TRIANGLE_STRIP, 0, 4));
        if self.textured {
            assert_eq!(self.tex_env_mode, gles11::MODULATE as GLint);
        }
        self.draws.push(Draw {
            texture: self.textured.then_some(self.texture),
            color: self.color,
        });
    }

    unsafe fn MatrixMode(&mut self, _mode: GLenum) {}
    unsafe fn PushMatrix(&mut self) {}
    unsafe fn PopMatrix(&mut self) {}
    unsafe fn LoadIdentity(&mut self) {}
    unsafe fn Orthof(
        &mut self,
        _left: GLfloat,
        _right: GLfloat,
        _bottom: GLfloat,
        _top: GLfloat,
        _near: GLfloat,
        _far: GLfloat,
    ) {
    }
    unsafe fn EnableClientState(&mut self, _array: GLenum) {}
    unsafe fn DisableClientState(&mut self, _array: GLenum) {}
    unsafe fn BlendFunc(&mut self, _sfactor: GLenum, _dfactor: GLenum) {}
    unsafe fn VertexPointer(
        &mut self,
        _size: GLint,
        _type: GLenum,
        _stride: GLsizei,
        _pointer: *const GLvoid,
    ) {
    }
    unsafe fn TexCoordPointer(
        &mut self,
        _size: GLint,
        _type: GLenum,
        _stride: GLsizei,
        _pointer: *const GLvoid,
    ) {
    }
}

#[test]
fn gles1_overlay_does_not_sample_the_presented_game_frame() {
    const GAME_TEXTURE: GLuint = 7;
    const ATLAS_TEXTURE: GLuint = 8;
    let mut quads = Vec::new();
    push_rect(
        &mut quads,
        Rect {
            x: 10.0,
            y: 10.0,
            w: 32.0,
            h: 32.0,
        },
        COL_ACCENT,
    );
    let button = quads[0];
    let glyph = Quad {
        tex: ATLAS_TEXTURE,
        col: COL_TEXT,
        ..button
    };
    // Initial solids, repeated glyphs, then a panel and another glyph.
    quads.extend([button, glyph, glyph, button, glyph]);

    // No cursor/FPS leaves texturing on; those overlays can leave it off.
    for textured in [true, false] {
        for viewport in [(0, 100, 320, 480), (100, 0, 480, 320)] {
            let mut gles = RecordingGles {
                textured,
                texture: GAME_TEXTURE,
                tex_env_mode: gles11::REPLACE as GLint,
                color: (1.0, 1.0, 1.0, 1.0),
                draws: Vec::new(),
            };
            unsafe { render_gles1(&mut gles, viewport, &quads) };
            let expected: Vec<_> = quads
                .iter()
                .map(|q| Draw {
                    texture: (q.tex != 0).then_some(q.tex),
                    color: q.col,
                })
                .collect();
            assert_eq!(gles.draws, expected);
            assert_eq!(gles.texture, GAME_TEXTURE);
            assert_eq!(gles.tex_env_mode, gles11::REPLACE as GLint);
            assert!(!gles.textured);
        }
    }
}

#[test]
fn panel_has_unique_actions_and_results_header_below_keypad() {
    let mut ui = TrainerUi::new();
    ui.open = true;
    for viewport in [(0, 100, 320, 480), (100, 0, 480, 320)] {
        let panel = compute_layout(&ui, viewport).panel.unwrap();
        let ids: std::collections::HashSet<_> = panel.widgets.iter().map(|&(id, _)| id).collect();
        assert_eq!(ids.len(), panel.widgets.len(), "duplicate widget IDs");
        assert!(panel.rect.y + panel.rect.h <= viewport.3 as f32);
        for &(id, rect) in &panel.widgets {
            assert!(rect.w > 0.0 && rect.h > 0.0, "invalid widget {id}");
            assert!(rect.y >= panel.rect.y && rect.y + rect.h <= panel.rect.y + panel.rect.h);
        }
        for id in [W_CATEGORY] {
            assert_eq!(
                panel
                    .widgets
                    .iter()
                    .filter(|&&(widget, _)| widget == id)
                    .count(),
                1
            );
        }
        assert_eq!(
            panel
                .widgets
                .iter()
                .filter(|&&(id, _)| id == W_SET_ALL)
                .count(),
            1
        );
        assert_eq!(
            panel
                .widgets
                .iter()
                .filter(|&&(id, _)| id == W_DUMP)
                .count(),
            1
        );
        let toggle = panel
            .widgets
            .iter()
            .find(|&&(id, _)| id == W_SAFE_MODE)
            .unwrap()
            .1;
        let type_button = panel
            .widgets
            .iter()
            .find(|&&(id, _)| id == W_TYPE)
            .unwrap()
            .1;
        assert_eq!(toggle.y, type_button.y);
        assert!(toggle.x >= type_button.x + type_button.w);
        assert!(toggle.x + toggle.w <= panel.rect.x + panel.rect.w);
        let scroll = panel
            .widgets
            .iter()
            .find(|&&(id, _)| id == W_SCROLL_UP)
            .unwrap()
            .1;
        assert_eq!(panel.results_header_y, scroll.y);
        for &(id, rect) in &panel.widgets {
            if (W_KEY_BASE..W_KEY_BASE + 16).contains(&id) {
                assert!(rect.y + rect.h <= panel.results_header_y);
            }
        }
    }
}

#[test]
fn dump_and_set_all_dispatch_distinct_commands() {
    let _guard = INPUT_TEST_LOCK.lock().unwrap();
    let mut ui = TrainerUi::new();
    ui.set_text = "999".to_string();
    take_commands();
    activate_widget(&mut ui, W_DUMP);
    let commands = take_commands();
    assert!(matches!(
        commands.as_slice(),
        [TrainerCmd::CancelBulk, TrainerCmd::Dump]
    ));
    activate_widget(&mut ui, W_SET_ALL);
    let commands = take_commands();
    assert!(matches!(
        commands.as_slice(),
        [TrainerCmd::SetAll { confirm: false, .. }]
    ));
    ui.bulk_preview = true;
    activate_widget(&mut ui, W_SET_ALL);
    let commands = take_commands();
    assert!(matches!(
        commands.as_slice(),
        [TrainerCmd::SetAll { confirm: true, .. }]
    ));
    // A second click before a new preview is drawn cannot confirm again.
    activate_widget(&mut ui, W_SET_ALL);
    let commands = take_commands();
    assert!(matches!(
        commands.as_slice(),
        [TrainerCmd::SetAll { confirm: false, .. }]
    ));
    ui.bulk_preview = true;
    activate_widget(&mut ui, W_TYPE);
    assert!(!ui.bulk_preview);
    assert!(matches!(
        take_commands().as_slice(),
        [TrainerCmd::CancelBulk]
    ));
}

#[test]
fn safe_mode_latches_and_is_sent_with_bulk_commands() {
    let _guard = INPUT_TEST_LOCK.lock().unwrap();
    let mut ui = TrainerUi::new();
    ui.set_text = "999".to_string();
    assert!(ui.safe_mode);
    assert_eq!(
        safe_mode_colors(ui.safe_mode),
        (COL_SAFE_MODE_ON, COL_SAFE_MODE_TEXT)
    );
    take_commands();

    ui.bulk_preview = true;
    activate_widget(&mut ui, W_SAFE_MODE);
    assert!(!ui.safe_mode);
    assert!(!ui.bulk_preview);
    assert_eq!(safe_mode_colors(ui.safe_mode), (COL_WIDGET, COL_TEXT));
    assert!(matches!(
        take_commands().as_slice(),
        [TrainerCmd::CancelBulk]
    ));
    activate_widget(&mut ui, W_SET_ALL);
    assert!(matches!(
        take_commands().as_slice(),
        [TrainerCmd::SetAll {
            safe_mode: false,
            confirm: false,
            ..
        }]
    ));
    assert!(!ui.safe_mode);

    activate_widget(&mut ui, W_SAFE_MODE);
    assert!(ui.safe_mode);
    // Other controls and closing/reopening the panel do not unlatch it.
    for id in [W_TYPE, W_FIELD_SET, W_CLOSE, W_BUTTON, W_RESET] {
        activate_widget(&mut ui, id);
        assert!(ui.safe_mode);
        assert_eq!(safe_mode_colors(ui.safe_mode).0, COL_SAFE_MODE_ON);
    }
    take_commands();
    activate_widget(&mut ui, W_SET_ALL);
    assert!(matches!(
        take_commands().as_slice(),
        [TrainerCmd::SetAll {
            safe_mode: true,
            confirm: false,
            ..
        }]
    ));
    assert!(ui.safe_mode);
}

fn category_results(count: usize) -> Vec<SearchResult> {
    (0..count)
        .map(|i| {
            let mut analysis = crate::trainer::classify::Analysis::default();
            analysis.category = if i < 250 {
                Category::Unknown
            } else {
                Category::Money
            };
            SearchResult {
                addr: 0x1000 + i as u32 * 4,
                vtype: VType::I32,
                bits: 135,
                changed: false,
                analysis,
            }
        })
        .collect()
}

#[test]
fn categories_and_paging_use_all_hits_not_just_the_first_two_hundred() {
    let _guard = INPUT_TEST_LOCK.lock().unwrap();
    let hits = category_results(300);
    let mut ui = TrainerUi::new();
    ui.update_results(&hits, true);
    assert_eq!(ui.total_results, 300);
    assert_eq!(ui.category_counts[Category::Money.index()], 50);
    assert_eq!(ui.category_counts[Category::Unknown.index()], 250);
    assert_eq!(ui.results.len(), RESULT_ROWS);
    take_commands();
    activate_widget(&mut ui, W_CATEGORY);
    assert_eq!(ui.filter, ResultFilter::Category(Category::Money));
    assert!(
        ui.results.is_empty(),
        "stale page must not remain selectable"
    );
    assert!(matches!(
        take_commands().as_slice(),
        [TrainerCmd::CancelBulk, TrainerCmd::RefreshView]
    ));
    ui.update_results(&hits, false);
    assert_eq!(ui.filtered_results, 50);
    assert_eq!(ui.results[0].addr, hits[250].addr);
    activate_widget(&mut ui, W_SCROLL_DOWN);
    ui.update_results(&hits, false);
    assert_eq!(ui.results[0].addr, hits[255].addr);
    ui.scroll = usize::MAX;
    ui.update_results(&hits, false);
    assert_eq!(ui.results.last().unwrap().addr, hits[299].addr);
    ui.update_results(&[], false);
    assert_eq!(ui.scroll, 0);
    assert!(ui.results.is_empty());
    take_commands();
}

#[test]
fn category_selection_cancels_preview_and_scopes_bulk_command() {
    let _guard = INPUT_TEST_LOCK.lock().unwrap();
    let mut ui = TrainerUi::new();
    ui.bulk_preview = true;
    ui.selected = Some(0x1234);
    ui.selected_type = Some(VType::I32);
    take_commands();
    activate_widget(&mut ui, W_CATEGORY);
    assert!(!ui.bulk_preview);
    assert_eq!(ui.selected, None);
    assert_eq!(ui.selected_type, None);
    assert!(matches!(
        take_commands().as_slice(),
        [TrainerCmd::CancelBulk, TrainerCmd::RefreshView]
    ));
    activate_widget(&mut ui, W_SET_ALL);
    assert!(matches!(
        take_commands().as_slice(),
        [TrainerCmd::SetAll {
            filter: ResultFilter::Category(Category::Money),
            confirm: false,
            safe_mode: true,
            ..
        }]
    ));
}

#[test]
fn selected_result_keeps_concrete_type_and_shows_reason() {
    let _guard = INPUT_TEST_LOCK.lock().unwrap();
    let mut hits = category_results(1);
    hits.push(SearchResult {
        vtype: VType::U8,
        ..hits[0]
    });
    let mut ui = TrainerUi::new();
    ui.update_results(&hits, true);
    take_commands();
    activate_widget(&mut ui, W_RESULT_BASE + 1);
    assert_eq!(ui.selected, Some(hits[1].addr));
    assert_eq!(ui.selected_type, Some(VType::U8));
    assert!(ui.status.contains(hits[1].analysis.description()));
    ui.update_results(&hits, false);
    assert_eq!(ui.selected_type, Some(VType::U8));
    ui.update_results(&hits[..1], false);
    assert_eq!(ui.selected, None);
    take_commands();
}

#[test]
fn watch_window_is_independent_and_fits_portrait_and_landscape() {
    let mut ui = TrainerUi::new();
    ui.watch_open = true;
    for viewport in [(0, 100, 320, 480), (100, 0, 480, 320), (0, 0, 1920, 1080)] {
        let layout = compute_layout(&ui, viewport);
        assert!(layout.panel.is_none());
        let monitor = layout.monitor.unwrap();
        assert!(monitor.rect.x + monitor.rect.w <= viewport.2 as f32);
        assert!(monitor.rect.y + monitor.rect.h <= viewport.3 as f32);
        let ids: std::collections::HashSet<_> = monitor.widgets.iter().map(|w| w.0).collect();
        assert_eq!(ids.len(), monitor.widgets.len());
        for &(_, r) in &monitor.widgets {
            assert!(r.x >= monitor.rect.x && r.x + r.w <= monitor.rect.x + monitor.rect.w);
            assert!(r.y >= monitor.rect.y && r.y + r.h <= monitor.rect.y + monitor.rect.h);
        }
    }
    ui.open = true;
    for viewport in [(0, 0, 320, 480), (100, 0, 480, 320)] {
        let layout = compute_layout(&ui, viewport);
        let panel = layout.panel.unwrap();
        let monitor = layout.monitor.unwrap();
        assert!(monitor.rect.x + monitor.rect.w <= panel.rect.x);
    }
}

#[test]
fn watch_controls_pause_browse_and_select_without_writing() {
    let _guard = INPUT_TEST_LOCK.lock().unwrap();
    let mut ui = TrainerUi::new();
    ui.open = true;
    ui.activity = (0..12)
        .map(|i| Change {
            addr: 0x1000 + 4 * i,
            vtype: VType::I32,
            before: 30,
            after: 29,
            at: std::time::Instant::now(),
        })
        .collect();
    take_commands();
    activate_widget(&mut ui, W_WATCH);
    assert!(ui.watch_open && ui.open);
    activate_widget(&mut ui, W_WATCH_OLDER);
    assert!(ui.watch_paused);
    assert_eq!(ui.activity_scroll, RESULT_ROWS);
    take_commands();
    activate_widget(&mut ui, W_CHANGE_BASE);
    assert!(ui.open);
    assert_eq!(ui.watch_target, Some((0x1014, VType::I32)));
    assert!(matches!(
        take_commands().as_slice(),
        [TrainerCmd::CancelBulk, TrainerCmd::RefreshView]
    ));
    activate_widget(&mut ui, W_WATCH_PAUSE);
    assert!(!ui.watch_paused);
    assert_eq!(ui.activity_scroll, 0);
    take_commands();
    activate_widget(&mut ui, W_MARK);
    assert!(matches!(
        take_commands().as_slice(),
        [TrainerCmd::CancelBulk, TrainerCmd::Mark]
    ));
    activate_widget(&mut ui, W_DECREASED);
    assert!(matches!(
        take_commands().as_slice(),
        [
            TrainerCmd::CancelBulk,
            TrainerCmd::Compare(WatchFilter::Decreased)
        ]
    ));
}

#[test]
fn watch_edit_is_pinned_and_independent_of_main_selection_and_live_feed() {
    let _guard = INPUT_TEST_LOCK.lock().unwrap();
    let mut ui = TrainerUi::new();
    ui.selected = Some(0x9999);
    ui.selected_type = Some(VType::F32);
    ui.activity.push(Change {
        addr: 0x1000,
        vtype: VType::I32,
        before: 30,
        after: 29,
        at: std::time::Instant::now(),
    });
    take_commands();
    activate_widget(&mut ui, W_CHANGE_BASE);
    assert!(!ui.open, "WATCH must not open the main editor");
    assert_eq!(ui.selected, Some(0x9999));
    assert_eq!(ui.watch_target, Some((0x1000, VType::I32)));
    ui.activity[0].addr = 0x2000; // incoming feed reordered
    ui.watch_current = Some(28);
    activate_widget(&mut ui, W_WATCH_KEY_BASE + 7); // CLR
    activate_widget(&mut ui, W_WATCH_KEY_BASE + 8); // 7
    assert_eq!(ui.watch_text, "7");
    take_commands();
    activate_widget(&mut ui, W_WATCH_SET);
    assert!(
        matches!(take_commands().as_slice(), [TrainerCmd::CancelBulk,
        TrainerCmd::WatchSet { addr: 0x1000, vtype: VType::I32, text }] if text == "7")
    );
    ui.watch_current = None;
    activate_widget(&mut ui, W_WATCH_SET);
    assert!(matches!(
        take_commands().as_slice(),
        [TrainerCmd::CancelBulk]
    ));
    activate_widget(&mut ui, W_WATCH_DONE);
    assert_eq!(ui.watch_target, None);
    take_commands();
}

#[test]
fn watch_inline_editor_fits_and_both_windows_have_distinct_hit_targets() {
    let mut ui = TrainerUi::new();
    ui.watch_open = true;
    ui.watch_target = Some((0x1000, VType::I32));
    for open in [false, true] {
        ui.open = open;
        for viewport in [(0, 100, 320, 480), (100, 0, 480, 320), (0, 0, 1920, 1080)] {
            let layout = compute_layout(&ui, viewport);
            let mut ids = std::collections::HashSet::new();
            for panel in layout.panels() {
                assert!(panel.rect.y + panel.rect.h <= viewport.3 as f32);
                assert!(panel.rect.x + panel.rect.w <= viewport.2 as f32);
                for &(id, r) in &panel.widgets {
                    assert!(ids.insert(id), "duplicate id {id}");
                    assert!(
                        r.x >= panel.rect.x && r.x + r.w <= panel.rect.x + panel.rect.w + 0.001
                    );
                    assert!(
                        r.y >= panel.rect.y && r.y + r.h <= panel.rect.y + panel.rect.h + 0.001
                    );
                    let x = r.x + r.w / 2.0;
                    let y = r.y + r.h / 2.0;
                    assert_eq!(
                        layout
                            .panels()
                            .flat_map(|p| &p.widgets)
                            .filter(|(_, rect)| rect.contains(x, y))
                            .count(),
                        1
                    );
                }
            }
            assert!(ids.contains(&W_WATCH_SET));
            assert!(ids.contains(&(W_WATCH_KEY_BASE + 8)));
        }
    }
}
