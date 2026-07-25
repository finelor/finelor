use crate::web::components::ui::{
    AccountingTable, Alert, AlertKind, ButtonKind, Card, DocumentStateBadges, Eyebrow, Grid,
    GridCols, GridItem, GridSpan, HiddenPlaceholder, InfoItem, Inline, Justify, KeyValueList,
    KeyValueRow, LinkButton, LoadingCard, MutedAccountingCells, PageTitle, ReceiptPreview, Space,
    Stack, ValidationText,
};
use crate::web::server::dashboard::{AccountingRow, DocumentDetails, get_document_details};
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;

#[component]
pub fn DocumentDetailsPage() -> impl IntoView {
    let params = use_params_map();
    let short_ref = Memo::new(move |_| params.read().get("short_ref").unwrap_or_default());

    let page_data = Resource::new(
        move || short_ref.get(),
        |ref_value| async move { get_document_details(ref_value).await },
    );

    view! {
        <Stack>
            <Suspense fallback=|| view! { <LoadingCard message="Loading details..." /> }>
                {move || match page_data.get() {
                    Some(Ok(Some(doc))) => render_doc_details(doc).into_any(),
                    Some(Ok(None)) => view! {
                        <Alert kind=AlertKind::Error>
                            "Document not found for this company."
                        </Alert>
                    }
                    .into_any(),
                    Some(Err(err)) => {
                        view! {
                            <Alert kind=AlertKind::Error>
                                {format!("Failed to load details: {}", err)}
                            </Alert>
                        }
                        .into_any()
                    }
                    None => view! { <HiddenPlaceholder /> }.into_any(),
                }}
            </Suspense>
        </Stack>
    }
}

fn render_doc_details(doc: DocumentDetails) -> impl IntoView {
    let has_image = doc.image_url.is_some()
        && doc
            .mime_type
            .clone()
            .unwrap_or_default()
            .starts_with("image/");
    let image_src = doc.image_url.clone().unwrap_or_default();

    let accounting_rows = if doc.accounting_rows.is_empty() {
        vec![
            view! {
                <MutedAccountingCells />
            }
            .into_any(),
        ]
    } else {
        doc.accounting_rows
            .iter()
            .map(render_accounting_row)
            .collect::<Vec<_>>()
    };

    view! {
        <>
            <Inline justify=Justify::End>
                <LinkButton href="/transactions".to_string() kind=ButtonKind::Ghost>
                    "Back to month"
                </LinkButton>
            </Inline>

            <Alert kind=AlertKind::Success>
                <PageTitle>"Document Details"</PageTitle>
                <DocumentStateBadges
                    intake_status=doc.status.intake.status.clone().unwrap_or_default()
                    accounting_status=doc.status.accounting.status.clone().unwrap_or_default()
                />
                {doc.short_ref.clone()}
            </Alert>

            <Grid columns=GridCols::Detail section=true>
                <GridItem span=GridSpan::Wide>
                    <Card>
                        <ReceiptPreview
                            has_image=has_image
                            image_src=image_src
                            filename=doc.filename.clone().unwrap_or_else(|| "unknown".to_string())
                        />
                    </Card>
                </GridItem>

                <Stack gap=Space::Md section=true>
                    <Card>
                        <Eyebrow>"Receipt summary"</Eyebrow>
                        <KeyValueList>
                            <KeyValueRow label="Date" value={doc.transaction_date.clone().unwrap_or_else(|| "-".to_string())} />
                            <KeyValueRow label="Vendor" value={doc.supplier_name.clone().unwrap_or_else(|| "-".to_string())} />
                            <KeyValueRow label="Total" value={doc.total_amount.clone().unwrap_or_else(|| "-".to_string())} />
                            <KeyValueRow label="VAT" value={doc.vat_amount.clone().unwrap_or_else(|| "-".to_string())} />
                            <KeyValueRow label="Subtotal" value={doc.subtotal_amount.clone().unwrap_or_else(|| "-".to_string())} />
                        </KeyValueList>
                    </Card>

                    <Card>
                        <Eyebrow>"Pipeline"</Eyebrow>
                        <KeyValueList>
                            <KeyValueRow label="Model" value={doc.model_used.clone().unwrap_or_else(|| "-".to_string())} />
                            <KeyValueRow label="AI confidence" value={doc.ai_confidence.map(|c| format!("{:.0}%", c * 100.0)).unwrap_or_else(|| "-".to_string())} />
                            <KeyValueRow label="Review decision" value={doc.review_decision_type.clone().unwrap_or_else(|| "-".to_string())} />
                        </KeyValueList>
                    </Card>
                </Stack>
            </Grid>

            <Grid columns=GridCols::Detail section=true>
                <GridItem span=GridSpan::Wide>
                    <Card>
                        <Eyebrow>"Extracted data"</Eyebrow>
                        <Grid columns=GridCols::Two top=Space::Sm>
                            <InfoItem label="Document type" value={doc.document_type.clone().unwrap_or_else(|| "-".to_string())} />
                            <InfoItem label="Account code" value={doc.assigned_account_code.clone().unwrap_or_else(|| "-".to_string())} />
                            <InfoItem label="Account name" value={doc.account_name.clone().unwrap_or_else(|| "-".to_string())} />
                            <InfoItem label="Net amount" value={doc.net_amount.clone().unwrap_or_else(|| "-".to_string())} />
                        </Grid>
                    </Card>
                </GridItem>

                <Card>
                    <Eyebrow>"Validation"</Eyebrow>
                    <ValidationText
                        summary=doc.review_reason.clone().unwrap_or_else(|| "No review reason recorded".to_string())
                        details=doc.validation_errors.map(|v| v.to_string()).unwrap_or_else(|| "No validation errors".to_string())
                    />
                </Card>
            </Grid>

            <Card>
                <Eyebrow>"Accounting verification"</Eyebrow>
                <AccountingTable rows=accounting_rows />
            </Card>
        </>
    }
}

fn render_accounting_row(row: &AccountingRow) -> AnyView {
    view! {
        <tr>
            <td>{row.account_code.clone()}</td>
            <td>{row.description.clone().unwrap_or_else(|| "-".to_string())}</td>
            <td>{row.amount.clone().unwrap_or_else(|| "-".to_string())}</td>
            <td>{row.vat_code.clone().unwrap_or_else(|| "-".to_string())}</td>
        </tr>
    }
    .into_any()
}
