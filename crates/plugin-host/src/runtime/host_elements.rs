//! `elements`: the client's own elements by name.

use crate::bindings::wit;
use crate::capability::Capability;
use crate::model::{self, ElementInsert, ElementOp, ElementStyle, Place, PressMode, UiError};

use super::convert::{self, color_from_wit};
use super::state::{PluginState, valid_id};

type ElementResult = wasmtime::Result<Result<(), wit::elements::ElementError>>;

fn to_wit(e: UiError) -> wit::elements::ElementError {
    use wit::elements::ElementError as E;
    match e {
        UiError::Invalid => E::Invalid,
        UiError::Limit => E::Limit,
        _ => E::Unavailable,
    }
}

impl PluginState {
    fn element_op(&mut self, call: &str, op: ElementOp) -> ElementResult {
        self.require(Capability::Ui)?;
        self.main_only(call)?;
        let r = self.backend.lock().unwrap().element_update(&self.id, op);
        Ok(r.map_err(to_wit))
    }
}

impl wit::wire_raw::Host for PluginState {
    fn set_work(&mut self, work: String) -> wasmtime::Result<Result<(), String>> {
        self.require(Capability::WireRaw)?;
        self.main_only("wire-raw.set-work")?;
        let work = if work.trim().is_empty() {
            None
        } else {
            match model::raw_work(&self.id, &work) {
                Ok(w) => Some(w),
                Err(e) => return Ok(Err(e)),
            }
        };
        let r = self.backend.lock().unwrap().set_raw_work(&self.id, work);
        Ok(r.map_err(|e| e.to_string()))
    }

    fn selector(&mut self, name: String) -> wasmtime::Result<Option<String>> {
        self.require(Capability::WireRaw)?;
        Ok(self.backend.lock().unwrap().element_selector(&name))
    }
}

impl wit::elements::Host for PluginState {
    fn names(&mut self) -> wasmtime::Result<Vec<String>> {
        self.require(Capability::Ui)?;
        let mut names = self.backend.lock().unwrap().elements();
        names.sort();
        Ok(names)
    }

    fn hide(&mut self, name: String) -> ElementResult {
        self.element_op("elements.hide", ElementOp::Hide { name })
    }

    fn style(&mut self, name: String, style: wit::elements::ElementStyle) -> ElementResult {
        let fin =
            |x: Option<f32>, lo: f32, hi: f32| x.filter(|v| v.is_finite()).map(|v| v.clamp(lo, hi));
        let style = ElementStyle {
            background: style.background.map(color_from_wit),
            foreground: style.foreground.map(color_from_wit),
            border: style.border.map(color_from_wit),
            opacity: fin(style.opacity, 0.0, 1.0),
            radius: fin(style.radius, 0.0, 64.0),
        };
        self.element_op("elements.style", ElementOp::Style { name, style })
    }

    fn on_press(&mut self, name: String, mode: wit::elements::PressMode) -> ElementResult {
        use wit::elements::PressMode as M;
        let mode = match mode {
            M::Observe => PressMode::Observe,
            M::Consume => PressMode::Consume,
            M::Retarget(to) => PressMode::Retarget(to),
        };
        self.element_op("elements.on-press", ElementOp::OnPress { name, mode })
    }

    fn insert(
        &mut self,
        id: String,
        name: String,
        at: wit::elements::Place,
        elements: Vec<wit::types::Element>,
    ) -> ElementResult {
        if !valid_id(&id) || elements.is_empty() {
            self.require(Capability::Ui)?;
            return Ok(Err(wit::elements::ElementError::Invalid));
        }
        let place = match at {
            wit::elements::Place::Before => Place::Before,
            wit::elements::Place::After => Place::After,
            wit::elements::Place::Inside => Place::Inside,
        };
        let elements: Vec<model::Element> = convert::elements_from_wit(elements);
        self.element_op(
            "elements.insert",
            ElementOp::Insert(ElementInsert {
                id,
                name,
                place,
                elements,
            }),
        )
    }

    fn remove(&mut self, id: String) -> wasmtime::Result<()> {
        self.element_op("elements.remove", ElementOp::Remove { id })
            .map(|_| ())
    }

    fn reset(&mut self, name: String) -> wasmtime::Result<()> {
        self.element_op("elements.reset", ElementOp::Reset { name })
            .map(|_| ())
    }
}
