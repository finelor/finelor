use crate::web::components::ui::{Badge, Tone};
use icondata::Icon as IconData;
use leptos::prelude::*;
use leptos_icons::Icon;

#[component]
pub fn Metric(
    label: &'static str,
    value: String,
    #[prop(optional)] desc: Option<&'static str>,
) -> impl IntoView {
    view! {
        <div class="card card-border bg-base-100 border-base-300 shadow-sm">
            <div class="stat">
                <div class="stat-title">{label}</div>
                <div class="stat-value">{value}</div>
                <Show when=move || desc.is_some()>
                    <div class="stat-desc">{desc.unwrap_or_default()}</div>
                </Show>
            </div>
        </div>
    }
}

#[component]
pub fn InfoItem(
    label: &'static str,
    value: String,
    #[prop(optional)] icon: Option<IconData>,
) -> impl IntoView {
    if let Some(icon) = icon {
        view! {
            <div class="flex items-start gap-3">
                <Icon icon=icon width="1rem" height="1rem" attr:class="mt-1 text-base-content/45" />
                <div class="min-w-0">
                    <p class="text-xs font-bold uppercase tracking-widest text-base-content/45">{label}</p>
                    <p class="break-words text-sm font-semibold text-base-content">{value}</p>
                </div>
            </div>
        }
        .into_any()
    } else {
        view! {
            <div>
                <p class="text-xs text-base-content/60">{label}</p>
                <p class="text-sm font-semibold text-base-content">{value}</p>
            </div>
        }
        .into_any()
    }
}

#[component]
pub fn KeyValueList(children: Children) -> impl IntoView {
    view! { <dl class="mt-3 space-y-2 text-sm">{children()}</dl> }
}

#[component]
pub fn KeyValueRow(label: &'static str, value: String) -> impl IntoView {
    view! {
        <div class="flex justify-between gap-3">
            <dt class="text-base-content/65">{label}</dt>
            <dd class="text-right text-base-content">{value}</dd>
        </div>
    }
}

#[component]
pub fn PreviewRows(children: Children) -> impl IntoView {
    view! { <div class="space-y-2">{children()}</div> }
}

#[component]
pub fn PlainList(children: Children) -> impl IntoView {
    view! { <ul class="mt-4 space-y-2 text-sm text-base-content/70">{children()}</ul> }
}

#[component]
pub fn PlainListItem(children: Children) -> impl IntoView {
    view! { <li>{children()}</li> }
}

#[component]
pub fn DashboardPreviewRow(
    href: String,
    title: String,
    reference: String,
    amount: String,
    date: String,
) -> impl IntoView {
    view! {
        <a href=href class="flex items-center justify-between px-3 py-2">
            <div class="min-w-0">
                <p class="truncate text-sm font-semibold text-base-content">{title}</p>
                <p class="text-xs text-base-content/60">{reference}</p>
            </div>
            <div class="text-right">
                <p class="text-sm font-semibold text-base-content">{amount}</p>
                <p class="text-xs text-base-content/60">{date}</p>
            </div>
        </a>
    }
}

#[component]
pub fn TransactionsHeader() -> impl IntoView {
    view! {
        <div class="mb-2 hidden grid-cols-7 gap-3 px-1 text-xs font-bold uppercase tracking-widest text-base-content/45 md:grid">
            <p>"Date"</p>
            <p class="md:col-span-2">"Description"</p>
            <p>"Amount"</p>
            <p>"Direction"</p>
            <p>"Status"</p>
            <p>"AI confidence"</p>
        </div>
    }
}

#[component]
pub fn TransactionRow(
    href: String,
    date: String,
    title: String,
    reference: String,
    amount: String,
    status: String,
    confidence: String,
) -> impl IntoView {
    view! {
        <a href=href class="grid grid-cols-2 gap-3 p-3 md:grid-cols-7 md:items-center">
            <p class="text-sm text-base-content/65">{date}</p>
            <div class="min-w-0 md:col-span-2">
                <p class="truncate text-sm font-semibold text-base-content">{title}</p>
                <p class="text-xs text-base-content/60">{reference}</p>
            </div>
            <p class="text-sm font-semibold text-base-content">{amount}</p>
            <p class="text-sm text-base-content/65">"Money Out"</p>
            <Badge tone=match status.as_str() {
                "EXPORT_READY" | "EXPORTED" => Tone::Success,
                "PENDING_HUMAN_REVIEW" => Tone::Warning,
                "FAILED" => Tone::Error,
                _ => Tone::Info,
            }>{status}</Badge>
            <p class="text-sm text-base-content/65">{confidence}</p>
        </a>
    }
}

#[component]
pub fn ReceiptPreview(has_image: bool, image_src: String, filename: String) -> impl IntoView {
    let image = if has_image {
        view! {
            <img
                src=image_src
                alt="Receipt"
                class="h-[380px] w-full object-contain"
                loading="lazy"
            />
        }
        .into_any()
    } else {
        view! {
            <div class="flex h-[380px] items-center justify-center text-sm text-base-content/65">
                "Image unavailable"
            </div>
        }
        .into_any()
    };

    view! {
        <>
            <div class="mb-3 flex items-center justify-between gap-3">
                <p class="text-xs font-bold uppercase tracking-widest text-base-content/45">"Receipt"</p>
                <p class="truncate text-xs text-base-content/65">{filename}</p>
            </div>
            {image}
        </>
    }
}

#[component]
pub fn AccountingTable(rows: Vec<AnyView>) -> impl IntoView {
    view! {
        <div class="mt-3 overflow-x-auto">
            <table class="table">
                <thead>
                    <tr>
                        <th>"Account"</th>
                        <th>"Description"</th>
                        <th>"Amount"</th>
                        <th>"VAT"</th>
                    </tr>
                </thead>
                <tbody>{rows}</tbody>
            </table>
        </div>
    }
}

#[component]
pub fn MutedAccountingCells() -> impl IntoView {
    view! {
        <tr>
            <td class="text-base-content/60">"-"</td>
            <td class="text-base-content/60">"No accounting rows yet"</td>
            <td class="text-base-content/60">"-"</td>
            <td class="text-base-content/60">"-"</td>
        </tr>
    }
}

#[component]
pub fn ValidationText(summary: String, details: String) -> impl IntoView {
    view! {
        <>
            <p class="mt-2 text-sm text-base-content">{summary}</p>
            <p class="mt-2 break-words text-xs text-base-content/65">{details}</p>
        </>
    }
}
