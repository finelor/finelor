use crate::web::components::ui::{
    Alert, AlertKind, Banner, BannerProminence, Card, EmptyState, FormField, Grid, GridCols,
    HiddenPlaceholder, LoadingCard, Metric, MonthInput, PreviewRows, SelectInput, Stack, TextInput,
    Tone, TransactionRow, TransactionsHeader,
};
use crate::web::server::auth::{TransactionsResponse, get_document_list};
use icondata::{LuCircleCheck, LuShieldCheck};
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
enum TransactionsPageState {
    Loaded(TransactionsResponse),
}

#[component]
pub fn Transactions() -> impl IntoView {
    let search_query = RwSignal::new(String::new());
    let status_filter = RwSignal::new(String::new());
    let month_filter = RwSignal::new(String::new());

    let page_data = Resource::new(
        move || (search_query.get(), status_filter.get(), month_filter.get()),
        |(search, status, month)| async move {
            Ok::<_, ServerFnError>(TransactionsPageState::Loaded(
                get_document_list(
                    if month.trim().is_empty() {
                        None
                    } else {
                        Some(month)
                    },
                    if status.trim().is_empty() {
                        None
                    } else {
                        Some(status)
                    },
                    if search.trim().is_empty() {
                        None
                    } else {
                        Some(search)
                    },
                    Some(100),
                    Some(0),
                )
                .await?,
            ))
        },
    );

    view! {
        <Stack>
            <Suspense fallback=|| view! { <LoadingCard message="Loading transactions..." /> }>
                {move || match page_data.get() {
                    Some(Ok(TransactionsPageState::Loaded(data))) => {
                        let rows = if data.items.is_empty() {
                            view! { <EmptyState>"No transactions found for current filters."</EmptyState> }
                            .into_any()
                        } else {
                            let items = data
                                .items
                                .iter()
                                .map(|doc| {
                                    view! {
                                        <TransactionRow
                                            href=format!("/transactions/{}", doc.short_ref)
                                            date=doc.received_date.clone().unwrap_or_else(|| "-".to_string())
                                            title=doc.supplier_name.clone().unwrap_or_else(|| "Unknown supplier".to_string())
                                            reference=doc.short_ref.clone()
                                            amount=doc.total_amount.clone().unwrap_or_else(|| "-".to_string())
                                            status=doc.status.clone()
                                            confidence=doc.ai_confidence.map(|c| format!("{:.0}%", c * 100.0)).unwrap_or_else(|| "-".to_string())
                                        />
                                    }
                                })
                                .collect::<Vec<_>>();
                            view! { <PreviewRows>{items}</PreviewRows> }.into_any()
                        };

                        view! {
                            <>
                                <Banner
                                    title="Month review progress".to_string()
                                    subtitle=format!("{}% complete · {} documents", data.completion_rate, data.total_documents)
                                    icon=LuShieldCheck
                                    tone=Tone::Success
                                    prominence=BannerProminence::Spacious
                                    trailing_icon=LuCircleCheck
                                />

                                <Grid columns=GridCols::Four section=true>
                                    <Metric label="Month progress" value=format!("{}%", data.completion_rate) />
                                    <Metric label="Documents" value=data.total_documents.to_string() />
                                    <Metric label="Needs review" value=data.pending_documents.to_string() />
                                    <Metric label="Done" value=data.done_documents.to_string() />
                                </Grid>

                                <Card>
                                    <Grid columns=GridCols::Filter>
                                        <FormField label="Month">
                                            <MonthInput value=month_filter />
                                        </FormField>
                                        <FormField label="Status">
                                            <SelectInput value=status_filter>
                                                <option value="">"All statuses"</option>
                                                <option value="PENDING_HUMAN_REVIEW">"Pending review"</option>
                                                <option value="EXPORT_READY">"Export ready"</option>
                                                <option value="EXPORTED">"Exported"</option>
                                                <option value="FAILED">"Failed"</option>
                                            </SelectInput>
                                        </FormField>
                                        <FormField label="Search">
                                            <TextInput value=search_query placeholder="Search by ref or supplier" />
                                        </FormField>
                                    </Grid>
                                </Card>

                                <Card>
                                    <TransactionsHeader />
                                    {rows}
                                </Card>
                            </>
                        }
                        .into_any()
                    }
                    Some(Err(err)) => view! {
                        <Alert kind=AlertKind::Error>
                            {format!("Failed to load transactions: {}", err)}
                        </Alert>
                    }
                    .into_any(),
                    None => view! { <HiddenPlaceholder /> }.into_any(),
                }}
            </Suspense>
        </Stack>
    }
}
