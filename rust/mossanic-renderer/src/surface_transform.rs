use ash::vk;
use m2s2_math::Matrix4x4f32;

pub(super) fn swaps_extent(transform: vk::SurfaceTransformFlagsKHR) -> bool {
    transform == vk::SurfaceTransformFlagsKHR::ROTATE_90
        || transform == vk::SurfaceTransformFlagsKHR::ROTATE_270
}

pub(super) fn pre_rotation(transform: vk::SurfaceTransformFlagsKHR) -> Matrix4x4f32 {
    let values = if transform == vk::SurfaceTransformFlagsKHR::ROTATE_90 {
        [
            0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]
    } else if transform == vk::SurfaceTransformFlagsKHR::ROTATE_180 {
        [
            -1.0, 0.0, 0.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]
    } else if transform == vk::SurfaceTransformFlagsKHR::ROTATE_270 {
        [
            0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]
    } else {
        [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ]
    };
    Matrix4x4f32::from_slice(&values)
}

#[cfg(test)]
mod tests {
    use super::{pre_rotation, swaps_extent};
    use ash::vk::SurfaceTransformFlagsKHR as Transform;

    #[test]
    fn swaps_extent_only_for_quarter_turns() {
        assert!(!swaps_extent(Transform::IDENTITY));
        assert!(swaps_extent(Transform::ROTATE_90));
        assert!(!swaps_extent(Transform::ROTATE_180));
        assert!(swaps_extent(Transform::ROTATE_270));
    }

    #[test]
    fn maps_quarter_turns_to_clip_space_rotation_matrices() {
        assert_eq!(
            pre_rotation(Transform::ROTATE_90).as_slice(),
            &[0.0, -1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,]
        );
        assert_eq!(
            pre_rotation(Transform::ROTATE_270).as_slice(),
            &[0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,]
        );
    }
}
