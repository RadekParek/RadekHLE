use super::gles1_on_gles2_logging::GLES1to2Logger;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RotationFixMode {
    None,
    Clockwise,
    CounterClockwise,
}

impl RotationFixMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Clockwise => "clockwise",
            Self::CounterClockwise => "counter-clockwise",
        }
    }
}

pub fn rotation_fix_mode() -> RotationFixMode {
    match std::env::var("TOUCHHLE_GLES1_ROTATION_FIX") {
        Ok(value)
            if value.eq_ignore_ascii_case("cw") || value.eq_ignore_ascii_case("clockwise") =>
        {
            RotationFixMode::Clockwise
        }
        Ok(value)
            if value.eq_ignore_ascii_case("ccw")
                || value.eq_ignore_ascii_case("counter-clockwise")
                || value.eq_ignore_ascii_case("counterclockwise") =>
        {
            RotationFixMode::CounterClockwise
        }
        _ => RotationFixMode::None,
    }
}

pub struct MatrixFixer;

impl MatrixFixer {
    pub fn apply_all_fixes(matrix: &mut [f32; 16], logger: &GLES1to2Logger) -> [f32; 16] {
        let original = *matrix;
        logger.log_matrix("original", &original, true);
        let mode = rotation_fix_mode();

        Self::log_fix(logger, 1, "axis swap candidate retained for diagnosis");
        Self::log_fix(logger, 2, "Y inversion candidate retained for diagnosis");
        Self::log_fix(
            logger,
            3,
            &format!("screen rotation mode={}", mode.as_str()),
        );
        Self::log_fix(
            logger,
            4,
            "transpose candidate rejected for column-major matrices",
        );

        if mode != RotationFixMode::None {
            let rotation = match mode {
                RotationFixMode::None => unreachable!(),
                RotationFixMode::Clockwise => screen_rotation_clockwise(),
                RotationFixMode::CounterClockwise => screen_rotation_counter_clockwise(),
            };
            *matrix = multiply(&rotation, matrix);
        }

        Self::log_fix(logger, 5, "Z axis preserved");
        Self::log_fix(
            logger,
            6,
            "scale preserved; no normalisation of guest transforms",
        );
        Self::log_fix(logger, 7, "perspective terms preserved");
        logger.log_matrix("fixed", matrix, false);
        *matrix
    }

    fn log_fix(logger: &GLES1to2Logger, number: u8, message: &str) {
        if super::gles1_on_gles2_logging::enabled() {
            log!(
                "[GLES1→GLES2 FIX] op={} fix={}/7 {}",
                operation_id(logger),
                number,
                message
            );
        }
    }
}

fn operation_id(logger: &GLES1to2Logger) -> u64 {
    logger.operation_id()
}

fn screen_rotation_clockwise() -> [f32; 16] {
    [
        0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

fn screen_rotation_counter_clockwise() -> [f32; 16] {
    [
        0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

fn multiply(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut result = [0.0; 16];
    for column in 0..4 {
        for row in 0..4 {
            result[column * 4 + row] = (0..4)
                .map(|index| a[index * 4 + row] * b[column * 4 + index])
                .sum();
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_matrices_are_inverse_pairs() {
        assert_eq!(
            multiply(
                &screen_rotation_clockwise(),
                &screen_rotation_counter_clockwise()
            ),
            identity()
        );
        assert_eq!(
            multiply(
                &screen_rotation_counter_clockwise(),
                &screen_rotation_clockwise()
            ),
            identity()
        );
    }

    #[test]
    fn rotation_preserves_depth_and_homogeneous_coordinates() {
        let matrix = screen_rotation_counter_clockwise();
        assert_eq!(
            [matrix[2], matrix[6], matrix[10], matrix[14]],
            [0.0, 0.0, 1.0, 0.0]
        );
        assert_eq!(
            [matrix[3], matrix[7], matrix[11], matrix[15]],
            [0.0, 0.0, 0.0, 1.0]
        );
    }

    fn identity() -> [f32; 16] {
        [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]
    }
}
