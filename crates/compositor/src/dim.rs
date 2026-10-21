//! Per-surface colour dimming in the window render path.
use smithay::{
    backend::renderer::{
        Frame, Renderer,
        element::{Element, Id, Kind, RenderElement},
        utils::{CommitCounter, DamageSet, OpaqueRegions},
    },
    utils::{Buffer, Physical, Rectangle, Scale, Transform},
};

#[derive(Debug)]
pub struct DimElement<E> {
    pub surface: E,
    pub brightness: f32,
    pub revision: usize,
}

impl<E: Element> Element for DimElement<E> {
    fn id(&self) -> &Id {
        self.surface.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.surface
            .current_commit()
            .distance(Some(CommitCounter::default()))
            .unwrap_or_default()
            .wrapping_add(self.revision)
            .into()
    }
    fn src(&self) -> Rectangle<f64, Buffer> {
        self.surface.src()
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.surface.geometry(scale)
    }
    fn transform(&self) -> Transform {
        self.surface.transform()
    }
    fn damage_since(&self, scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        if commit == Some(self.current_commit()) {
            DamageSet::default()
        } else {
            DamageSet::from_slice(&[Rectangle::from_size(self.geometry(scale).size)])
        }
    }
    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        self.surface.opaque_regions(scale)
    }
    fn alpha(&self) -> f32 {
        self.surface.alpha()
    }
    fn kind(&self) -> Kind {
        self.surface.kind()
    }
}

impl<R: Renderer, E: RenderElement<R>> RenderElement<R> for DimElement<E> {
    #[expect(
        clippy::float_arithmetic,
        reason = "black source-over multiplies the rendered surface colour"
    )]
    fn draw(
        &self,
        frame: &mut R::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), R::Error> {
        self.surface.draw(frame, src, dst, damage, opaque_regions)?;
        if self.brightness < 1.0 {
            frame.draw_solid(dst, damage, [0.0, 0.0, 0.0, 1.0 - self.brightness].into())?;
        }
        Ok(())
    }
}
