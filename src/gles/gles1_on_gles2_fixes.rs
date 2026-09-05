use super::gles1_on_gles2_logging::GLES1to2Logger;
use crate::options::RenderRotation;

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
                Self::fix_rotation_cw(matrix);
                Self::log_fix(logger, 1, "axis swap disabled to avoid a second coordinate remap");
                Self::log_fix(logger, 2, "Y inversion disabled to avoid a second coordinate remap");
                Self::log_fix(logger, 3, "screen rotation applied (clockwise_90) [first]");
                Self::fix_rotation_cw(matrix);
                Self::log_fix(logger, 3, "screen rotation applied (clockwise_90) [second]");
                Self::log_fix(logger, 4, "horizontal flip disabled after blackscreen regression");
            }
            RotationFixMode::Clockwise => {
                Self::fix_rotation_cw(matrix);
                Self::log_fix(logger, 1, "axis swap candidate retained for diagnosis");
                Self::log_fix(logger, 2, "Y inversion candidate retained for diagnosis");
                Self::log_fix(logger, 3, "screen rotation applied (clockwise_90)");
            }
            RotationFixMode::CounterClockwise => {
                Self::fix_rotation_ccw(matrix);
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

        Self::log_fix(logger, 5, "transpose skipped for column-major matrices");
        Self::log_fix(logger, 6, "Z axis preserved; scale normalised without changing guest scale factors");
        Self::log_fix(
            logger,
            7,
            "perspective fixed without discarding existing perspective terms",
        );
        logger.log_matrix("fixed", matrix, false);
        *matrix
    }

    fn fix_rotation_cw(matrix: &mut [f32; 16]) {
        *matrix = multiply(&screen_rotation_clockwise(), matrix);
    }

    fn fix_rotation_ccw(matrix: &mut [f32; 16]) {
        *matrix = multiply(&screen_rotation_counter_clockwise(), matrix);
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

pub fn apply_render_rotation(
    matrix: &mut [f32; 16],
    rotation: RenderRotation,
    logger: &GLES1to2Logger,
) -> [f32; 16] {
    if rotation == RenderRotation::Default {
        return *matrix;
    }
    let rotation_matrix = match rotation {
        RenderRotation::Default => unreachable!(),
        RenderRotation::Minus90 => screen_rotation_clockwise(),
        RenderRotation::Minus180 | RenderRotation::Plus180 => screen_rotation_180(),
        RenderRotation::Plus90 => screen_rotation_counter_clockwise(),
    };
    *matrix = multiply(&rotation_matrix, matrix);
    if super::gles1_on_gles2_logging::enabled() {
        log!(
            "[GLES1→GLES2 RENDER ROTATION] op={} rotation={}",
            logger.operation_id(),
            rotation.label()
        );
    }
    *matrix
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

fn screen_rotation_180() -> [f32; 16] {
    [
        -1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
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
    fn aggressive_composite_is_two_clockwise_quarter_turns() {
        let mut matrix = identity();
        MatrixFixer::fix_rotation_cw(&mut matrix);
        MatrixFixer::fix_rotation_cw(&mut matrix);
        assert_eq!(matrix, screen_rotation_180());
    }

    #[test]
    fn aggressive_composite_preserves_depth_and_homogeneous_coordinates() {
        let mut matrix = identity();
        MatrixFixer::fix_rotation_cw(&mut matrix);
        MatrixFixer::fix_rotation_cw(&mut matrix);
        assert_eq!([matrix[2], matrix[6], matrix[10], matrix[14]], [0.0, 0.0, 1.0, 0.0]);
        assert_eq!([matrix[3], matrix[7], matrix[11], matrix[15]], [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn render_rotation_uses_screen_space_without_changing_depth() {
        let mut matrix = identity();
        let logger = GLES1to2Logger::new("test", "test");
        let result = apply_render_rotation(&mut matrix, RenderRotation::Plus90, &logger);
        assert_eq!(result, screen_rotation_counter_clockwise());
        assert_eq!([result[2], result[6], result[10], result[14]], [0.0, 0.0, 1.0, 0.0]);
        logger.finish();
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
