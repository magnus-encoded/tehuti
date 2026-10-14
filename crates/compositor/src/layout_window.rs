//! Assigned layout geometry stays independent of client resize commits.
use core::cell::{Cell, RefCell};
use std::rc::Rc;

use smithay::{
    backend::renderer::{ImportAll, Renderer, element::AsRenderElements},
    desktop::{Window, space::SpaceElement},
    output::Output,
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale, Size},
};

type Deferred = (Output, Rectangle<i32, Logical>);

#[derive(Debug, Clone)]
pub struct LayoutWindow {
    window: Window,
    size: Rc<Cell<Size<i32, Logical>>>,
    size_held: Rc<Cell<bool>>,
    /// Output overlaps reported while the surface had no buffer yet.
    deferred: Rc<RefCell<Vec<Deferred>>>,
}

impl LayoutWindow {
    pub fn new(window: Window) -> Self {
        Self {
            window,
            size: Rc::new(Cell::new(Size::default())),
            size_held: Rc::new(Cell::new(false)),
            deferred: Rc::new(RefCell::new(Vec::new())),
        }
    }

    pub fn layout_size(&self) -> Size<i32, Logical> {
        self.size.get()
    }

    /// Whether the size hint in the first configure is being withheld.
    pub fn size_held(&self) -> bool {
        self.size_held.get()
    }

    pub fn hold_size(&self, held: bool) {
        self.size_held.set(held);
    }

    /// Smithay's output tracking sends `wl_surface.leave` for a surface that
    /// has no buffer yet, which would undo the early enter that clients such
    /// as mpv need to size their first buffer. Hold overlaps back until mapped.
    pub fn mapped(&self) -> bool {
        self.window.toplevel().is_some_and(|toplevel| {
            smithay::backend::renderer::utils::with_renderer_surface_state(toplevel.wl_surface(), |state| {
                state.buffer().is_some()
            })
            .unwrap_or(false)
        })
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
        if self.mapped() {
            self.window.output_enter(output, overlap);
        } else {
            let mut deferred = self.deferred.borrow_mut();
            deferred.retain(|(held, _)| held != output);
            deferred.push((output.clone(), overlap));
        }
    }

    fn output_leave(&self, output: &Output) {
        self.deferred.borrow_mut().retain(|(held, _)| held != output);
        self.window.output_leave(output);
    }

    fn refresh(&self) {
        if !self.mapped() {
            return;
        }
        let deferred = core::mem::take(&mut *self.deferred.borrow_mut());
        for (output, overlap) in deferred {
            self.window.output_enter(&output, overlap);
        }
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
