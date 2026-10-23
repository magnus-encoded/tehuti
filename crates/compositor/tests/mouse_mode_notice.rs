#[cfg(test)]
mod tests {
    use smithay::{
        backend::renderer::element::memory::MemoryRenderBufferRenderElement,
        backend::renderer::{element::Element, pixman::PixmanRenderer},
        output::{Mode, Output, PhysicalProperties, Subpixel},
        reexports::wayland_server::Display,
        utils::Scale,
    };
    use tehuti::{
        input::{SyntheticPad, route},
        scale,
        state::Compositor,
    };
    use tehuti_protocol::{ButtonName, ButtonState, Event};

    #[test]
    fn persistent_mouse_chip_is_small_and_clear_of_both_bars() -> Result<(), Box<dyn core::error::Error>> {
        let display = Display::<Compositor>::new()?;
        let mut compositor = Compositor::new(&display.handle())?;
        let output = Output::new(
            "test".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Example".into(),
                model: "Test".into(),
            },
        );
        scale::apply(
            &output,
            Mode {
                size: (1024, 600).into(),
                refresh: 60_000,
            },
            1,
        );
        let mut pad = SyntheticPad::default();
        for _ in 0..2 {
            for state in [ButtonState::Pressed, ButtonState::Released] {
                pad.push(Event::Button {
                    name: ButtonName::Select,
                    state,
                });
            }
        }
        route(&mut compositor, &mut pad)?;
        let transient = notice_bounds(&compositor, &output)?;
        assert_eq!(transient.loc, (16, 16).into());
        assert_eq!(transient.size, (512, 64).into());
        std::thread::sleep(core::time::Duration::from_millis(3100));
        let bounds = notice_bounds(&compositor, &output)?;
        assert!(
            bounds.size.h <= 600 / 20,
            "chip exceeds 5% of output height: {bounds:?}"
        );
        assert!(bounds.loc.y > 32, "chip touches the top menu bar");
        assert!(bounds.loc.y >= 450, "chip belongs in a bottom corner");
        assert!(bounds.loc.x > 0 && bounds.loc.x + bounds.size.w < 256);
        assert!(bounds.loc.y + bounds.size.h < 600 - tehuti_core::bar::HEIGHT);
        Ok(())
    }

    fn notice_bounds(
        compositor: &Compositor,
        output: &Output,
    ) -> Result<smithay::utils::Rectangle<i32, smithay::utils::Physical>, Box<dyn core::error::Error>> {
        let (buffer, location) = compositor.notice_for_output(output).ok_or("missing notice")?;
        let mut renderer = PixmanRenderer::new()?;
        let element = MemoryRenderBufferRenderElement::from_buffer(
            &mut renderer,
            location,
            &buffer,
            None,
            None,
            None,
            smithay::backend::renderer::element::Kind::Unspecified,
        )?;
        Ok(element.geometry(Scale::from(1.0)))
    }
}
