//! An unchanged output draws nothing; a change redraws only what it touched.

#[cfg(test)]
mod redraw {
    use core::error::Error;
    use smithay::{
        backend::{
            allocator::Fourcc,
            renderer::{Bind, Offscreen, damage::OutputDamageTracker, pixman::PixmanRenderer},
        },
        output::{Mode, Output, PhysicalProperties, Scale, Subpixel},
        reexports::wayland_server::Display,
        utils::{Physical, Rectangle, Transform},
    };
    use tehuti::{overlays::Overlays, state::Compositor, wallpaper::WallpaperCache};

    type Damage = Vec<Rectangle<i32, Physical>>;

    const WIDTH: i32 = 800;
    const HEIGHT: i32 = 600;

    fn output() -> Output {
        let output = Output::new(
            "redraw-test".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "Tehuti".into(),
                model: "Test".into(),
            },
        );
        output.change_current_state(
            Some(Mode {
                size: (WIDTH, HEIGHT).into(),
                refresh: 60_000,
            }),
            Some(Transform::Normal),
            Some(Scale::Integer(1)),
            Some((0, 0).into()),
        );
        output
    }

    struct Screen {
        renderer: PixmanRenderer,
        buffers: [smithay::reexports::pixman::Image<'static, 'static>; 2],
        damage: OutputDamageTracker,
        overlays: Overlays,
        cache: std::sync::Mutex<WallpaperCache>,
        output: Output,
        display: Display<Compositor>,
    }

    impl Screen {
        fn new() -> Result<Self, Box<dyn Error>> {
            let mut renderer = PixmanRenderer::new()?;
            let buffers = [
                renderer.create_buffer(Fourcc::Abgr8888, (WIDTH, HEIGHT).into())?,
                renderer.create_buffer(Fourcc::Abgr8888, (WIDTH, HEIGHT).into())?,
            ];
            let output = output();
            Ok(Self {
                renderer,
                buffers,
                damage: OutputDamageTracker::from_output(&output),
                overlays: Overlays::default(),
                cache: std::sync::Mutex::new(WallpaperCache::new("/nonexistent/wallpaper.png".into())),
                output,
                display: Display::new()?,
            })
        }

        /// Draws into `buffer` as a buffer `age` frames old; the drawn damage, or none.
        fn draw(
            &mut self,
            compositor: &Compositor,
            buffer: usize,
            age: usize,
            clock: &str,
        ) -> Result<Option<Damage>, Box<dyn Error>> {
            let overlay = self
                .overlays
                .elements(&mut self.renderer, &self.output, compositor, clock)?;
            let mut target = self.renderer.bind(&mut self.buffers[buffer])?;
            let frame = tehuti::wallpaper::render_output_with_cache_at_age(
                &self.output,
                &mut self.renderer,
                &mut target,
                compositor,
                &overlay.elements,
                &mut self.damage,
                &self.cache,
                age,
            )?;
            let damage = frame.damage.cloned();
            frame.sync.wait()?;
            Ok(damage)
        }
    }

    fn bar_rows() -> Rectangle<i32, Physical> {
        let top = HEIGHT.saturating_sub(tehuti_core::bar::HEIGHT);
        Rectangle::new((0, top).into(), (WIDTH, tehuti_core::bar::HEIGHT).into())
    }

    fn within(rectangles: &[Rectangle<i32, Physical>], area: Rectangle<i32, Physical>) -> bool {
        rectangles
            .iter()
            .all(|rectangle| rectangle.intersection(area) == Some(*rectangle))
    }

    #[test]
    fn an_unchanged_output_draws_nothing() -> Result<(), Box<dyn Error>> {
        let mut screen = Screen::new()?;
        let compositor = Compositor::new(&screen.display.handle())?;
        assert!(
            screen.draw(&compositor, 0, 0, "12:00")?.is_some(),
            "the first frame draws the whole output"
        );
        for _ in 0..3 {
            assert_eq!(
                screen.draw(&compositor, 0, 1, "12:00")?,
                None,
                "nothing changed, so nothing is drawn"
            );
        }
        Ok(())
    }

    #[test]
    fn a_new_minute_redraws_only_the_bar() -> Result<(), Box<dyn Error>> {
        let mut screen = Screen::new()?;
        let compositor = Compositor::new(&screen.display.handle())?;
        let _first = screen.draw(&compositor, 0, 0, "12:00")?;
        let damage = screen
            .draw(&compositor, 0, 1, "12:01")?
            .ok_or("the clock changed, so the bar is drawn")?;
        assert!(!damage.is_empty(), "the bar is damaged");
        assert!(within(&damage, bar_rows()), "only the bar changed: {damage:?}");
        Ok(())
    }

    #[test]
    fn a_buffer_two_frames_old_catches_up_on_the_other_buffers_change() -> Result<(), Box<dyn Error>> {
        let mut screen = Screen::new()?;
        let compositor = Compositor::new(&screen.display.handle())?;
        let _front = screen.draw(&compositor, 0, 0, "12:00")?;
        let _back = screen.draw(&compositor, 1, 0, "12:00")?;
        let _changed = screen
            .draw(&compositor, 0, 2, "12:01")?
            .ok_or("the clock changed")?;
        let catch_up = screen
            .draw(&compositor, 1, 2, "12:01")?
            .ok_or("the other buffer still shows the old minute")?;
        assert!(
            catch_up.iter().any(|rectangle| rectangle.overlaps(bar_rows())),
            "the bar is redrawn in the stale buffer: {catch_up:?}"
        );
        Ok(())
    }
}
