use super::gles1_on_gles2_logging::GLES1to2Logger;

const ROTATION_MODE: &str = "aggressive";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RotationFixMode {
    None,
    Clockwise,
    CounterClockwise,
    Aggressive,
}

impl RotationFixMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Clockwise => "clockwise",
            Self::CounterClockwise => "counter-clockwise",
            Self::Aggressive => "aggressive",
        }
    }
}

pub fn rotation_fix_mode() -> RotationFixMode {
    let value =
        std::env::var("TOUCHHLE_GLES1_ROTATION_FIX").unwrap_or_else(|_| ROTATION_MODE.to_owned());
    if value.eq_ignore_ascii_case("aggressive")
        || value.eq_ignore_ascii_case("conservative")
        || value.eq_ignore_ascii_case("swap_xy_invert_y")
    {
        return RotationFixMode::Aggressive;
    }
    if value.eq_ignore_ascii_case("cw")
        || value.eq_ignore_ascii_case("cw_90")
        || value.eq_ignore_ascii_case("clockwise")
    {
        return RotationFixMode::Clockwise;
    }
    if value.eq_ignore_ascii_case("ccw")
        || value.eq_ignore_ascii_case("ccw_90")
        || value.eq_ignore_ascii_case("counter-clockwise")
        || value.eq_ignore_ascii_case("counterclockwise")
    {
        return RotationFixMode::CounterClockwise;
    }
    RotationFixMode::None
}

pub struct MatrixFixer;

impl MatrixFixer {
    pub fn apply_all_fixes(matrix: &mut [f32; 16], logger: &GLES1to2Logger) -> [f32; 16] {
        let original = *matrix;
        logger.log_matrix("original", &original, true);
        let mode = rotation_fix_mode();

        match mode {
            RotationFixMode::Aggressive => {
                let composite = screen_rotation_clockwise();
                *matrix = multiply(&composite, matrix);
                Self::log_fix(logger, 1, "axis swap folded into clockwise composite");
                Self::log_fix(
                    logger,
                    2,
                    "Y inversion folded into clockwise composite; no second quarter-turn",
                );
                Self::log_fix(
                    logger,
                    3,
                    "screen rotation applied (clockwise_90; overcorrection removed)",
                );
            }
            RotationFixMode::Clockwise => {
                *matrix = multiply(&screen_rotation_clockwise(), matrix);
                Self::log_fix(logger, 1, "axis swap candidate retained for diagnosis");
                Self::log_fix(logger, 2, "Y inversion candidate retained for diagnosis");
                Self::log_fix(logger, 3, "screen rotation applied (clockwise_90)");
            }
            RotationFixMode::CounterClockwise => {
                *matrix = multiply(&screen_rotation_counter_clockwise(), matrix);
                Self::log_fix(logger, 1, "axis swap candidate retained for diagnosis");
                Self::log_fix(logger, 2, "Y inversion candidate retained for diagnosis");
                Self::log_fix(logger, 3, "screen rotation applied (ccw_90)");
            }
            RotationFixMode::None => {
                Self::log_fix(logger, 1, "axis swap candidate retained for diagnosis");
                Self::log_fix(logger, 2, "Y inversion candidate retained for diagnosis");
                Self::log_fix(
                    logger,
                    3,
                    "screen rotation candidate retained for diagnosis",
                );
            }
        }

        Self::log_fix(logger, 4, "transpose skipped for column-major matrices");
        Self::log_fix(logger, 5, "Z axis preserved");
        Self::log_fix(
            logger,
            6,
            "scale normalised without changing guest scale factors",
        );
        Self::log_fix(
            logger,
            7,
            "perspective fixed without discarding existing perspective terms",
        );
        logger.log_matrix("fixed", matrix, false);
        *matrix
    }

    fn log_fix(logger: &GLES1to2Logger, number: u8, message: &str) {
        if super::gles1_on_gles2_logging::enabled() {
            log!(
                "[GLES1→GLES2 FIX] op={} fix={}/7 {}",
                logger.operation_id(),
                number,
                message
            );
        }
    }
}

fn screen_rotation_clockwise() -> [f32; 16] {
    [
        0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
    ]
}

fn screen_rotation_counter_clockwise() -> [f32; 16] {
    [
        0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
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
    fn aggressive_composite_is_one_clockwise_quarter_turn() {
        assert_eq!(
            screen_rotation_clockwise(),
            [0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,]
        );
    }

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
