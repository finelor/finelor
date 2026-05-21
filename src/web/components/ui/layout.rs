use leptos::prelude::*;

pub fn document_body_class() -> &'static str {
    "min-h-screen bg-base-200 text-base-content flex flex-col"
}

#[component]
pub fn AppCanvas(children: Children) -> impl IntoView {
    view! { <div class="min-h-screen bg-base-200 text-base-content flex-grow">{children()}</div> }
}

#[component]
pub fn PublicFrame(children: Children) -> impl IntoView {
    view! {
        <div class="flex min-h-screen flex-col">
            <main class="mx-auto w-full max-w-5xl flex-1 px-4 py-12 md:py-20">
                {children()}
            </main>
        </div>
    }
}

#[component]
pub fn ProtectedFrame(children: Children) -> impl IntoView {
    view! {
        <div class="flex min-h-screen flex-col">
            {children()}
        </div>
    }
}

#[component]
pub fn ProtectedMain(children: Children) -> impl IntoView {
    view! {
        <main class="mx-auto w-full max-w-6xl flex-1 px-4 py-6">
            {children()}
        </main>
    }
}

#[component]
pub fn Stack(
    children: Children,
    #[prop(default = Space::Lg)] gap: Space,
    #[prop(default = Space::None)] top: Space,
    #[prop(default = false)] section: bool,
    #[prop(default = false)] shrink: bool,
) -> impl IntoView {
    let class = stack_class(gap, top, shrink);
    if section {
        view! { <section class=class>{children()}</section> }.into_any()
    } else {
        view! { <div class=class>{children()}</div> }.into_any()
    }
}

#[derive(Clone, Copy)]
pub enum Space {
    None,
    Xs,
    Sm,
    Md,
    Lg,
    Xl,
}

#[derive(Clone, Copy)]
pub enum GridCols {
    Two,
    Three,
    Four,
    Five,
    Filter,
    InputAction,
    Sidebar,
    Detail,
    SplitTwo,
    EqualThreeCompact,
}

#[derive(Clone, Copy)]
pub enum GridSpan {
    Default,
    MediumWide,
    Wide,
}

#[component]
pub fn Grid(
    children: Children,
    columns: GridCols,
    #[prop(default = Space::Md)] gap: Space,
    #[prop(default = Space::None)] top: Space,
    #[prop(default = false)] section: bool,
) -> impl IntoView {
    let class = grid_class(columns, gap, top);
    if section {
        view! { <section class=class>{children()}</section> }.into_any()
    } else {
        view! { <div class=class>{children()}</div> }.into_any()
    }
}

#[component]
pub fn GridItem(
    children: Children,
    #[prop(default = GridSpan::Default)] span: GridSpan,
) -> impl IntoView {
    view! { <div class=grid_span_class(span)>{children()}</div> }
}

#[derive(Clone, Copy)]
pub enum Align {
    Start,
    Center,
}

#[derive(Clone, Copy)]
pub enum Justify {
    Start,
    End,
    Between,
}

#[derive(Clone, Copy)]
pub enum InlineSurface {
    Plain,
    Panel,
}

#[component]
pub fn Inline(
    children: Children,
    #[prop(default = Align::Center)] align: Align,
    #[prop(default = Justify::Start)] justify: Justify,
    #[prop(default = Space::Md)] gap: Space,
    #[prop(default = false)] wrap: bool,
    #[prop(default = false)] stack_mobile: bool,
    #[prop(default = false)] shrink: bool,
    #[prop(default = InlineSurface::Plain)] surface: InlineSurface,
) -> impl IntoView {
    view! { <div class=inline_class(align, justify, gap, wrap, stack_mobile, shrink, surface)>{children()}</div> }
}

#[component]
pub fn Cluster(
    children: Children,
    #[prop(default = Align::Center)] align: Align,
    #[prop(default = Space::Md)] gap: Space,
    #[prop(default = false)] shrink: bool,
) -> impl IntoView {
    view! { <div class=inline_class(align, Justify::Start, gap, true, false, shrink, InlineSurface::Plain)>{children()}</div> }
}

#[component]
pub fn Drawer(children: Children, drawer_id: &'static str) -> impl IntoView {
    view! {
        <div class="drawer">
            <input id=drawer_id type="checkbox" class="drawer-toggle" />
            {children()}
        </div>
    }
}

#[component]
pub fn DrawerContent(children: Children) -> impl IntoView {
    view! {
        <div class="drawer-content">
            {children()}
        </div>
    }
}

#[component]
pub fn SidebarLayout(children: Children) -> impl IntoView {
    view! {
        <section class="grid items-start gap-4 lg:grid-cols-[240px_minmax(0,1fr)] lg:[&>*]:mt-0">
            {children()}
        </section>
    }
}

#[component]
pub fn DesktopSidebar(children: Children) -> impl IntoView {
    view! {
        <aside class="hidden lg:block lg:sticky lg:top-24 lg:self-start">
            {children()}
        </aside>
    }
}

#[component]
pub fn ScrollContent(children: Children) -> impl IntoView {
    view! {
        <section class="block min-w-0 self-start lg:-m-2 lg:max-h-[calc(100vh-7rem)] lg:overflow-y-auto lg:p-2">
            {children()}
        </section>
    }
}

#[component]
pub fn DrawerSide(children: Children, drawer_id: &'static str) -> impl IntoView {
    view! {
        <div class="drawer-side z-50 lg:hidden">
            <label for=drawer_id aria-label="Close settings navigation" class="drawer-overlay"></label>
            <div class="h-dvh max-h-dvh w-80 overflow-hidden bg-base-100 p-5">
                {children()}
            </div>
        </div>
    }
}

fn stack_class(gap: Space, top: Space, shrink: bool) -> String {
    let mut classes = vec![space_y_class(gap)];
    if let Some(top_class) = margin_top_class(top) {
        classes.push(top_class);
    }
    if shrink {
        classes.push("min-w-0");
    }
    classes.join(" ")
}

fn grid_class(columns: GridCols, gap: Space, top: Space) -> String {
    let mut classes = match columns {
        GridCols::Two => vec!["grid", gap_class(gap), "md:grid-cols-2"],
        GridCols::Three => vec!["grid", gap_class(gap), "md:grid-cols-3"],
        GridCols::Four => vec!["grid", gap_class(gap), "md:grid-cols-4"],
        GridCols::Five => vec!["grid", gap_class(gap), "md:grid-cols-5"],
        GridCols::Filter => vec!["grid", gap_class(gap), "md:grid-cols-3"],
        GridCols::InputAction => vec!["grid", gap_class(gap), "md:grid-cols-[1fr_auto]"],
        GridCols::Sidebar => vec!["grid", gap_class(gap), "md:grid-cols-[240px_1fr]"],
        GridCols::Detail => vec!["grid", gap_class(gap), "lg:grid-cols-3"],
        GridCols::SplitTwo => vec![
            "mt-10",
            "grid",
            "items-start",
            gap_class(gap),
            "md:grid-cols-2",
        ],
        GridCols::EqualThreeCompact => vec!["grid", "grid-cols-3", gap_class(gap), "text-center"],
    };
    if let Some(top_class) = margin_top_class(top) {
        classes.push(top_class);
    }
    classes.join(" ")
}

fn grid_span_class(span: GridSpan) -> &'static str {
    match span {
        GridSpan::Default => "",
        GridSpan::MediumWide => "md:col-span-2",
        GridSpan::Wide => "lg:col-span-2",
    }
}

fn inline_class(
    align: Align,
    justify: Justify,
    gap: Space,
    wrap: bool,
    stack_mobile: bool,
    shrink: bool,
    surface: InlineSurface,
) -> String {
    let mut classes = if stack_mobile {
        vec!["flex", "flex-col", "sm:flex-row"]
    } else {
        vec!["flex"]
    };
    classes.push(match align {
        Align::Start => "items-start",
        Align::Center => "items-center",
    });
    classes.push(match justify {
        Justify::Start => "justify-start",
        Justify::End => "justify-end",
        Justify::Between => "justify-between",
    });
    classes.push(gap_class(gap));
    if wrap {
        classes.push("flex-wrap");
    }
    if shrink {
        classes.push("min-w-0");
    }
    if matches!(surface, InlineSurface::Panel) {
        classes.extend([
            "rounded-box",
            "border",
            "border-base-300",
            "bg-base-100",
            "p-4",
        ]);
    }
    classes.join(" ")
}

fn gap_class(space: Space) -> &'static str {
    match space {
        Space::None => "gap-0",
        Space::Xs => "gap-1",
        Space::Sm => "gap-2",
        Space::Md => "gap-3",
        Space::Lg => "gap-4",
        Space::Xl => "gap-6",
    }
}

fn space_y_class(space: Space) -> &'static str {
    match space {
        Space::None => "space-y-0",
        Space::Xs => "space-y-2",
        Space::Sm => "space-y-3",
        Space::Md => "space-y-4",
        Space::Lg => "space-y-6",
        Space::Xl => "space-y-8",
    }
}

fn margin_top_class(space: Space) -> Option<&'static str> {
    match space {
        Space::None => None,
        Space::Xs => Some("mt-2"),
        Space::Sm => Some("mt-3"),
        Space::Md => Some("mt-4"),
        Space::Lg => Some("mt-6"),
        Space::Xl => Some("mt-8"),
    }
}
