use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static INITIALIZATION_LOGGED: AtomicBool = AtomicBool::new(false);

pub(crate) fn enabled() -> bool {
    crate::gles::translator_tracing_enabled()
        || std::env::var_os("TOUCHHLE_GLES1_GLES2_LOG").is_some()
}

pub struct GLES1to2Logger {
    operation_id: u64,
    operation_name: String,
    started_at: Instant,
    context: String,
}

impl GLES1to2Logger {
    pub(crate) fn operation_id(&self) -> u64 {
        self.operation_id
    }

    pub fn new(operation_name: &str, context: &str) -> Self {
        Self {
            operation_id: crate::gles::next_gl_call_id(),
            operation_name: operation_name.to_owned(),
            started_at: Instant::now(),
            context: context.to_owned(),
        }
    }

    pub fn log_matrix(&self, label: &str, matrix: &[f32; 16], before_fix: bool) {
        if !enabled() {
            return;
        }
        let phase = if before_fix { "before" } else { "after" };
        log!(
            "[GLES1→GLES2 MATRIX] op={} name={} context={} phase={} label={}",
            self.operation_id,
            self.operation_name,
            self.context,
            phase,
            label
        );
        for row in 0..4 {
            log!(
                "[GLES1→GLES2 MATRIX] op={} row={} values=({:.6},{:.6},{:.6},{:.6})",
                self.operation_id,
                row,
                matrix[row],
                matrix[row + 4],
                matrix[row + 8],
                matrix[row + 12]
            );
        }
        let x_scale = column_length(matrix, 0);
        let y_scale = column_length(matrix, 1);
        let z_scale = column_length(matrix, 2);
        log!(
            "[GLES1→GLES2 MATRIX_PROPERTIES] op={} determinant={:.6} trace={:.6} scale=({:.6},{:.6},{:.6}) translation=({:.6},{:.6},{:.6})",
            self.operation_id,
            determinant(matrix),
            matrix[0] + matrix[5] + matrix[10] + matrix[15],
            x_scale,
            y_scale,
            z_scale,
            matrix[12],
            matrix[13],
            matrix[14]
        );
    }

    pub fn log_vertex_batch(&self, label: &str, vertices: &[[f32; 3]], sample_size: usize) {
        if !enabled() {
            return;
        }
        log!(
            "[GLES1→GLES2 VERTICES] op={} label={} total={} sample_size={}",
            self.operation_id,
            label,
            vertices.len(),
            sample_size.min(vertices.len())
        );
        for (index, vertex) in vertices.iter().take(sample_size).enumerate() {
            log!(
                "[GLES1→GLES2 VERTEX] op={} index={} value=({:.6},{:.6},{:.6})",
                self.operation_id,
                index,
                vertex[0],
                vertex[1],
                vertex[2]
            );
        }
    }

    pub fn log_vertex_transformation(&self, original: [f32; 3], transformed: [f32; 4]) {
        if !enabled() {
            return;
        }
        log!(
            "[GLES1→GLES2 VERTEX_TRANSFORM] op={} original=({:.6},{:.6},{:.6}) clip=({:.6},{:.6},{:.6},{:.6})",
            self.operation_id,
            original[0],
            original[1],
            original[2],
            transformed[0],
            transformed[1],
            transformed[2],
            transformed[3]
        );
        if transformed[3] != 0.0 {
            log!(
                "[GLES1→GLES2 NDC] op={} value=({:.6},{:.6},{:.6})",
                self.operation_id,
                transformed[0] / transformed[3],
                transformed[1] / transformed[3],
                transformed[2] / transformed[3]
            );
        }
    }

    pub fn log_rotation_operation(
        &self,
        angle: f32,
        axis: (f32, f32, f32),
        axis_after_fix: (f32, f32, f32),
    ) {
        if !enabled() {
            return;
        }
        let turns = (angle / 90.0).round();
        let is_right_angle = (angle - turns * 90.0).abs() < 0.01;
        log!(
            "[GLES1→GLES2 ROTATION] op={} angle={:.6} axis=({:.6},{:.6},{:.6}) axis_after_fix=({:.6},{:.6},{:.6}) right_angle={}",
            self.operation_id,
            angle,
            axis.0,
            axis.1,
            axis.2,
            axis_after_fix.0,
            axis_after_fix.1,
            axis_after_fix.2,
            is_right_angle
        );
    }

    pub fn log_viewport(
        &self,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        after_fix: Option<(i32, i32, u32, u32)>,
    ) {
        if !enabled() {
            return;
        }
        let before_aspect = aspect(width, height);
        log!(
            "[GLES1→GLES2 VIEWPORT] op={} before=({},{},{},{}) before_aspect={:.6}",
            self.operation_id,
            x,
            y,
            width,
            height,
            before_aspect
        );
        if let Some((fixed_x, fixed_y, fixed_width, fixed_height)) = after_fix {
            log!(
                "[GLES1→GLES2 VIEWPORT] op={} after=({},{},{},{}) after_aspect={:.6}",
                self.operation_id,
                fixed_x,
                fixed_y,
                fixed_width,
                fixed_height,
                aspect(fixed_width, fixed_height)
            );
        }
    }

    pub fn log_projection(
        &self,
        operation: &str,
        before: (f64, f64, f64, f64, f64, f64),
        after: Option<(f64, f64, f64, f64, f64, f64)>,
    ) {
        if !enabled() {
            return;
        }
        log!(
            "[GLES1→GLES2 PROJECTION] op={} kind={} before=({:.6},{:.6},{:.6},{:.6},{:.6},{:.6})",
            self.operation_id,
            operation,
            before.0,
            before.1,
            before.2,
            before.3,
            before.4,
            before.5
        );
        if let Some(after) = after {
            log!(
                "[GLES1→GLES2 PROJECTION] op={} kind={} after=({:.6},{:.6},{:.6},{:.6},{:.6},{:.6})",
                self.operation_id,
                operation,
                after.0,
                after.1,
                after.2,
                after.3,
                after.4,
                after.5
            );
        }
    }

    pub fn log_texture_coordinates(&self, unit: usize, coordinates: [f32; 4]) {
        if !enabled() {
            return;
        }
        log!(
            "[GLES1→GLES2 TEXCOORD] op={} unit={} value=({:.6},{:.6},{:.6},{:.6})",
            self.operation_id,
            unit,
            coordinates[0],
            coordinates[1],
            coordinates[2],
            coordinates[3]
        );
    }

    pub fn finish(&self) {
        if enabled() {
            log!(
                "[GLES1→GLES2 OPERATION] op={} name={} elapsed_us={}",
                self.operation_id,
                self.operation_name,
                self.started_at.elapsed().as_micros()
            );
        }
    }
}

pub fn log_initialization(rotation_mode: &str) {
    if !enabled() || INITIALIZATION_LOGGED.swap(true, Ordering::Relaxed) {
        return;
    }
    log!(
        "[GLES1→GLES2 INITIALIZED] rotation_mode={} logging=matrix,vertices,transformations,texture_coordinates,viewport,projection",
        rotation_mode
    );
}

fn aspect(width: u32, height: u32) -> f32 {
    if height == 0 {
        0.0
    } else {
        width as f32 / height as f32
    }
}

fn column_length(matrix: &[f32; 16], column: usize) -> f32 {
    let offset = column * 4;
    (matrix[offset].powi(2) + matrix[offset + 1].powi(2) + matrix[offset + 2].powi(2)).sqrt()
}

fn determinant(matrix: &[f32; 16]) -> f32 {
    let m = |row: usize, column: usize| matrix[column * 4 + row];
    m(0, 0)
        * (m(1, 1) * (m(2, 2) * m(3, 3) - m(2, 3) * m(3, 2))
            - m(1, 2) * (m(2, 1) * m(3, 3) - m(2, 3) * m(3, 1))
            + m(1, 3) * (m(2, 1) * m(3, 2) - m(2, 2) * m(3, 1)))
        - m(0, 1)
            * (m(1, 0) * (m(2, 2) * m(3, 3) - m(2, 3) * m(3, 2))
                - m(1, 2) * (m(2, 0) * m(3, 3) - m(2, 3) * m(3, 0))
                + m(1, 3) * (m(2, 0) * m(3, 2) - m(2, 2) * m(3, 0)))
        + m(0, 2)
            * (m(1, 0) * (m(2, 1) * m(3, 3) - m(2, 3) * m(3, 1))
                - m(1, 1) * (m(2, 0) * m(3, 3) - m(2, 3) * m(3, 0))
                + m(1, 3) * (m(2, 0) * m(3, 1) - m(2, 1) * m(3, 0)))
        - m(0, 3)
            * (m(1, 0) * (m(2, 1) * m(3, 2) - m(2, 2) * m(3, 1))
                - m(1, 1) * (m(2, 0) * m(3, 2) - m(2, 2) * m(3, 0))
                + m(1, 2) * (m(2, 0) * m(3, 1) - m(2, 1) * m(3, 0)))
}
