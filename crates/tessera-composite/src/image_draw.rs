//! Image-draw call shapes the compositor relies on, expressed over the
//! Optics v0.0.89 unified [`optics_gfx::ImageDrawOpts`] entry point
//! (Optics ADR-0175).
//!
//! Optics v0.0.89 folded its `draw_image_*` variants into a single
//! [`optics_gfx::CanvasCommands::draw_image_with`]. Each method here keeps
//! the exact semantics the compositor used before that change: source
//! sub-rectangles, opaque SRC-replace for alpha-free RGB, tint and blend from
//! an [`optics_gfx::ImageStyle`], and a circular (`curvature = 0.0`) rounded
//! clip. Curvature stays `0.0` so composited client surfaces render exactly
//! as they did on v0.0.88.

use optics_gfx::{CanvasCommands, CornerRadii, Image, ImageDrawOpts, ImageStyle};

pub(crate) trait ImageDrawExt {
    /// Draw a normalized `(u, v, du, dv)` sub-rectangle of `image` into `dst`.
    #[allow(clippy::too_many_arguments)]
    fn draw_image_sub(
        &self,
        image: &Image,
        dst_x: f32,
        dst_y: f32,
        dst_w: f32,
        dst_h: f32,
        src_u: f32,
        src_v: f32,
        src_du: f32,
        src_dv: f32,
    );

    /// Draw an alpha-free RGB image, forcing opaque output and replacing the
    /// destination.
    fn draw_image_opaque(&self, image: &Image, x: f32, y: f32, w: f32, h: f32);

    /// Draw a sub-rectangle of an alpha-free RGB image, SRC-replace with alpha
    /// forced opaque.
    #[allow(clippy::too_many_arguments)]
    fn draw_image_opaque_sub(
        &self,
        image: &Image,
        dst_x: f32,
        dst_y: f32,
        dst_w: f32,
        dst_h: f32,
        src_u: f32,
        src_v: f32,
        src_du: f32,
        src_dv: f32,
    );

    /// Draw an image with the tint and fixed-function blend in `style`.
    fn draw_image_with_style(
        &self,
        image: &Image,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        style: &ImageStyle,
    );

    /// Draw an image through one antialiased circular rounded clip that is
    /// independent of its destination.
    #[allow(clippy::too_many_arguments)]
    fn draw_image_clipped_rrect_with_style(
        &self,
        image: &Image,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        clip_x: f32,
        clip_y: f32,
        clip_w: f32,
        clip_h: f32,
        radius: f32,
        style: &ImageStyle,
    );
}

impl ImageDrawExt for CanvasCommands<'_> {
    fn draw_image_sub(
        &self,
        image: &Image,
        dst_x: f32,
        dst_y: f32,
        dst_w: f32,
        dst_h: f32,
        src_u: f32,
        src_v: f32,
        src_du: f32,
        src_dv: f32,
    ) {
        self.draw_image_with(
            image,
            (dst_x, dst_y, dst_w, dst_h),
            &ImageDrawOpts {
                src: Some((src_u, src_v, src_du, src_dv)),
                ..ImageDrawOpts::default()
            },
        );
    }

    fn draw_image_opaque(&self, image: &Image, x: f32, y: f32, w: f32, h: f32) {
        self.draw_image_with(
            image,
            (x, y, w, h),
            &ImageDrawOpts {
                opaque_only: true,
                ..ImageDrawOpts::default()
            },
        );
    }

    fn draw_image_opaque_sub(
        &self,
        image: &Image,
        dst_x: f32,
        dst_y: f32,
        dst_w: f32,
        dst_h: f32,
        src_u: f32,
        src_v: f32,
        src_du: f32,
        src_dv: f32,
    ) {
        self.draw_image_with(
            image,
            (dst_x, dst_y, dst_w, dst_h),
            &ImageDrawOpts {
                src: Some((src_u, src_v, src_du, src_dv)),
                opaque_only: true,
                ..ImageDrawOpts::default()
            },
        );
    }

    fn draw_image_with_style(
        &self,
        image: &Image,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        style: &ImageStyle,
    ) {
        self.draw_image_with(
            image,
            (x, y, w, h),
            &ImageDrawOpts {
                tint: style.tint,
                blend: style.blend,
                ..ImageDrawOpts::default()
            },
        );
    }

    fn draw_image_clipped_rrect_with_style(
        &self,
        image: &Image,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        clip_x: f32,
        clip_y: f32,
        clip_w: f32,
        clip_h: f32,
        radius: f32,
        style: &ImageStyle,
    ) {
        self.draw_image_with(
            image,
            (x, y, w, h),
            &ImageDrawOpts {
                clip: Some((clip_x, clip_y, clip_w, clip_h)),
                clip_radii: CornerRadii::uniform(radius),
                clip_curvature: 0.0,
                tint: style.tint,
                blend: style.blend,
                ..ImageDrawOpts::default()
            },
        );
    }
}
