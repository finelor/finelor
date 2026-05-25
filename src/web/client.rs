pub fn redirect(path: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        let window = leptos_dom::helpers::window();
        let _ = window.location().set_href(path);
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = path;
    }
}

pub struct AppEventSubscription {
    #[cfg(target_arch = "wasm32")]
    event_source: web_sys::EventSource,
    #[cfg(target_arch = "wasm32")]
    _on_open: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>,
    #[cfg(target_arch = "wasm32")]
    _on_error: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::Event)>,
    #[cfg(target_arch = "wasm32")]
    _on_message: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::MessageEvent)>,
}

impl Drop for AppEventSubscription {
    fn drop(&mut self) {
        #[cfg(target_arch = "wasm32")]
        self.event_source.close();
    }
}

pub fn subscribe_app_events<F, O, E>(
    on_event: F,
    on_open: O,
    on_error: E,
) -> Option<AppEventSubscription>
where
    F: Fn(String) + 'static,
    O: Fn() + 'static,
    E: Fn() + 'static,
{
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::{JsCast, closure::Closure};

        let event_source = web_sys::EventSource::new("/_events").ok()?;
        let on_open = Closure::wrap(Box::new(move |_event: web_sys::Event| {
            on_open();
        }) as Box<dyn FnMut(web_sys::Event)>);
        let on_error = Closure::wrap(Box::new(move |_event: web_sys::Event| {
            on_error();
        }) as Box<dyn FnMut(web_sys::Event)>);
        let on_message = Closure::wrap(Box::new(move |event: web_sys::MessageEvent| {
            if let Some(data) = event.data().as_string() {
                on_event(data);
            }
        }) as Box<dyn FnMut(web_sys::MessageEvent)>);

        event_source.set_onopen(Some(on_open.as_ref().unchecked_ref()));
        event_source.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        event_source.set_onmessage(Some(on_message.as_ref().unchecked_ref()));

        Some(AppEventSubscription {
            event_source,
            _on_open: on_open,
            _on_error: on_error,
            _on_message: on_message,
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = on_event;
        let _ = on_open;
        let _ = on_error;
        None
    }
}

pub struct IntervalSubscription {
    #[cfg(target_arch = "wasm32")]
    interval_id: i32,
    #[cfg(target_arch = "wasm32")]
    _callback: wasm_bindgen::closure::Closure<dyn FnMut()>,
}

impl Drop for IntervalSubscription {
    fn drop(&mut self) {
        #[cfg(target_arch = "wasm32")]
        leptos_dom::helpers::window().clear_interval_with_handle(self.interval_id);
    }
}

pub fn run_interval_ms<F>(delay_ms: i32, callback: F) -> Option<IntervalSubscription>
where
    F: FnMut() + 'static,
{
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::{JsCast, closure::Closure};

        let callback = Closure::wrap(Box::new(callback) as Box<dyn FnMut()>);
        let interval_id = leptos_dom::helpers::window()
            .set_interval_with_callback_and_timeout_and_arguments_0(
                callback.as_ref().unchecked_ref(),
                delay_ms,
            )
            .ok()?;

        Some(IntervalSubscription {
            interval_id,
            _callback: callback,
        })
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = delay_ms;
        let _ = callback;
        None
    }
}

pub fn copy_to_clipboard(text: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        let _ = leptos_dom::helpers::window()
            .navigator()
            .clipboard()
            .write_text(text);
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = text;
    }
}

pub fn run_after_ms<F>(delay_ms: i32, callback: F)
where
    F: FnOnce() + 'static,
{
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::{JsCast, closure::Closure};

        let callback = Closure::once(callback);
        let _ = leptos_dom::helpers::window()
            .set_timeout_with_callback_and_timeout_and_arguments_0(
                callback.as_ref().unchecked_ref(),
                delay_ms,
            );
        callback.forget();
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = delay_ms;
        let _ = callback;
    }
}

pub fn close_checkbox_drawer(id: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        use wasm_bindgen::JsCast;

        let document = leptos_dom::helpers::document();
        if let Some(input) = document
            .get_element_by_id(id)
            .and_then(|element| element.dyn_into::<web_sys::HtmlInputElement>().ok())
        {
            input.set_checked(false);
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = id;
    }
}
