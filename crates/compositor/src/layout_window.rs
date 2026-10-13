//! Assigned layout geometry stays independent of client resize commits.
use core::cell::Cell;
use std::rc::Rc;

use smithay::{
    backend::renderer::{ImportAll, Renderer, element::AsRenderElements},
    desktop::{Window, space::SpaceElement},
    output::Output,
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Size},
};

#[derive(Debug, Clone)]
pub struct LayoutWindow {
    window: Window,
    size: Rc<Cell<Size<i32, Logical>>>,
}

impl LayoutWindow {
    pub fn new(window: Window) -> Self {
        Self {
            window,
            size: Rc::new(Cell::new(Size::default())),
        }
    }

    pub fn set_size(&self, size: Size<i32, Logical>) {
        self.size.set(size);
    }
}

impl core::ops::Deref for LayoutWindow {
    type Target = Window;

    fn deref(&self) -> &Window {
        &self.window
    }
}

impl PartialEq for LayoutWindow {
    fn eq(&self, other: &Self) -> bool {
        self.window == other.window
    }
}

impl Eq for LayoutWindow {}

impl IsAlive for LayoutWindow {
    fn alive(&self) -> bool {
        self.window.alive()
    }
}

impl SpaceElement for LayoutWindow {
    fn geometry(&self) -> Rectangle<i32, Logical> {
        Rectangle::new(self.window.geometry().loc, self.size.get())
    }

    fn bbox(&self) -> Rectangle<i32, Logical> {
        self.window.bbox()
    }

    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        self.window.is_in_input_region(point)
    }

    fn z_index(&self) -> u8 {
        self.window.z_index()
    }

    fn set_activate(&self, activated: bool) {
        self.window.set_activate(activated);
    }

    fn output_enter(&self, output: &Output, overlap: Rectangle<i32, Logical>) {
        self.window.output_enter(output, overlap);
    }

    fn output_leave(&self, output: &Output) {
        self.window.output_leave(output);
    }

    fn refresh(&self) {
        self.window.refresh();
    }
}

impl<Render: Renderer + ImportAll> AsRenderElements<Render> for LayoutWindow
where
    Window: AsRenderElements<Render>,
{
    type RenderElement = <Window as AsRenderElements<Render>>::RenderElement;

    fn render_elements<Element: From<Self::RenderElement>>(
        &self,
        renderer: &mut Render,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<Element> {
        self.window.render_elements(renderer, location, scale, alpha)
    }
}
