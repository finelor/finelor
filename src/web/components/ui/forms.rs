use leptos::prelude::*;

#[component]
pub fn FormField(children: Children, label: &'static str) -> impl IntoView {
    view! {
        <fieldset class="fieldset">
            <label class="fieldset-label">{label}</label>
            {children()}
        </fieldset>
    }
}

#[component]
pub fn FormHelp(children: Children) -> impl IntoView {
    view! { <p class="label text-xs text-base-content/65">{children()}</p> }
}

#[component]
pub fn TextInput(
    #[prop(optional)] name: Option<&'static str>,
    #[prop(optional)] placeholder: Option<&'static str>,
    #[prop(default = false)] required: bool,
    #[prop(default = false)] disabled: bool,
    #[prop(optional)] value: Option<RwSignal<String>>,
    #[prop(optional)] on_input: Option<Callback<String>>,
) -> impl IntoView {
    match value {
        Some(value) => view! {
            <input
                type="text"
                name=name
                placeholder=placeholder
                class="input w-full"
                required=required
                disabled=disabled
                prop:value=move || value.get()
                on:input=move |ev| {
                    let next = leptos_dom::helpers::event_target_value(&ev);
                    value.set(next.clone());
                    if let Some(callback) = on_input {
                        callback.run(next);
                    }
                }
            />
        }
        .into_any(),
        None => view! {
            <input
                type="text"
                name=name
                placeholder=placeholder
                class="input w-full"
                required=required
                disabled=disabled
                value=""
                on:input=move |ev| {
                    if let Some(callback) = on_input {
                        callback.run(leptos_dom::helpers::event_target_value(&ev));
                    }
                }
            />
        }
        .into_any(),
    }
}

#[component]
pub fn EmailInput(
    #[prop(optional)] name: Option<&'static str>,
    #[prop(optional)] placeholder: Option<&'static str>,
    #[prop(default = false)] required: bool,
) -> impl IntoView {
    view! {
        <input type="email" name=name placeholder=placeholder class="input w-full" required=required value="" />
    }
}

#[component]
pub fn PasswordInput(
    #[prop(optional)] name: Option<&'static str>,
    #[prop(optional)] placeholder: Option<&'static str>,
    #[prop(default = false)] required: bool,
    #[prop(optional)] minlength: Option<&'static str>,
) -> impl IntoView {
    view! {
        <input
            type="password"
            name=name
            placeholder=placeholder
            class="input w-full"
            required=required
            minlength=minlength
            value=""
        />
    }
}

#[component]
pub fn MonthInput(
    #[prop(optional)] value: Option<RwSignal<String>>,
    #[prop(optional)] on_input: Option<Callback<String>>,
) -> impl IntoView {
    match value {
        Some(value) => view! {
            <input
                type="month"
                class="input w-full"
                prop:value=move || value.get()
                on:input=move |ev| {
                    let next = leptos_dom::helpers::event_target_value(&ev);
                    value.set(next.clone());
                    if let Some(callback) = on_input {
                        callback.run(next);
                    }
                }
            />
        }
        .into_any(),
        None => view! {
            <input
                type="month"
                class="input w-full"
                value=""
                on:input=move |ev| {
                    if let Some(callback) = on_input {
                        callback.run(leptos_dom::helpers::event_target_value(&ev));
                    }
                }
            />
        }
        .into_any(),
    }
}

#[component]
pub fn SelectInput(
    children: Children,
    #[prop(optional)] value: Option<RwSignal<String>>,
    #[prop(optional)] on_change: Option<Callback<String>>,
) -> impl IntoView {
    match value {
        Some(value) => view! {
            <select
                class="select w-full"
                prop:value=move || value.get()
                on:change=move |ev| {
                    let next = leptos_dom::helpers::event_target_value(&ev);
                    value.set(next.clone());
                    if let Some(callback) = on_change {
                        callback.run(next);
                    }
                }
            >
                {children()}
            </select>
        }
        .into_any(),
        None => view! {
            <select
                class="select w-full"
                on:change=move |ev| {
                    if let Some(callback) = on_change {
                        callback.run(leptos_dom::helpers::event_target_value(&ev));
                    }
                }
            >
                {children()}
            </select>
        }
        .into_any(),
    }
}
