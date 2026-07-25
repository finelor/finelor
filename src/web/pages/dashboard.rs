use crate::web::components::ui::{
    ActionCard, Alert, AlertKind, Banner, BannerProminence, BodyText, Button, ButtonKind,
    ButtonType, Card, CardTitle, DashboardPreviewRow, Eyebrow, FormField, Grid, GridCols, GridItem,
    GridSpan, HiddenPlaceholder, LoadingCard, Metric, PreviewRows, Stack, TextInput, TextLink,
    Tone,
};
use crate::web::server::dashboard::{
    CompleteCompanyOnboarding, DashboardSummary, get_dashboard_summary,
};
use icondata::{LuCircleCheck, LuShieldCheck};
use leptos::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
enum DashboardPageState {
    Loaded(DashboardSummary),
}

#[component]
pub fn Dashboard() -> impl IntoView {
    let complete_onboarding_action = ServerAction::<CompleteCompanyOnboarding>::new();
    let onboarding_company_name = RwSignal::new(String::new());
    let refresh_nonce = RwSignal::new(0_u64);

    let page_data = Resource::new(
        move || refresh_nonce.get(),
        |_| async move {
            Ok::<_, ServerFnError>(DashboardPageState::Loaded(get_dashboard_summary().await?))
        },
    );

    let complete_onboarding_value = complete_onboarding_action.value();
    Effect::new(move || {
        if let Some(Ok(())) = complete_onboarding_value.get() {
            onboarding_company_name.set(String::new());
            refresh_nonce.update(|nonce| *nonce += 1);
        }
    });

    view! {
        <Stack>
            <Suspense fallback=|| view! { <LoadingCard message="Loading dashboard..." /> }>
                {move || match page_data.get() {
                    Some(Ok(DashboardPageState::Loaded(summary))) => {
                        if summary.onboarding_required {
                            view! {
                                <Card>
                                    <Eyebrow>"Company setup"</Eyebrow>
                                    <CardTitle>"Welcome. Set your company name to continue."</CardTitle>
                                    <BodyText>"You can start with a name now and refine details later in Settings."</BodyText>

                                    <ActionForm action=complete_onboarding_action>
                                        <FormField label="Company name">
                                            <Grid columns=GridCols::InputAction>
                                                <TextInput
                                                    name="company_name"
                                                    value=onboarding_company_name
                                                    placeholder="Company name"
                                                    required=true
                                                />
                                                <Button kind=ButtonKind::Primary button_type=ButtonType::Submit>
                                                    "Save and continue"
                                                </Button>
                                            </Grid>
                                        </FormField>
                                    </ActionForm>

                                    <Show when=move || matches!(complete_onboarding_value.get(), Some(Err(_)))>
                                        <Alert kind=AlertKind::Error>
                                            {move || {
                                                complete_onboarding_value
                                                    .get()
                                                    .as_ref()
                                                    .and_then(|r| r.as_ref().err())
                                                    .map(|e| format!("Error: {}", e))
                                                    .unwrap_or_default()
                                            }}
                                        </Alert>
                                    </Show>
                                </Card>
                            }
                            .into_any()
                        } else {
                            let preview_rows = summary
                                .recent_transactions
                                .iter()
                                .map(|doc| {
                                    view! {
                                        <DashboardPreviewRow
                                            href=format!("/transactions/{}", doc.short_ref)
                                            title=doc.supplier_name.clone().unwrap_or_else(|| "Unknown supplier".to_string())
                                            reference=doc.short_ref.clone()
                                            amount=doc.total_amount.clone().unwrap_or_else(|| "-".to_string())
                                            date=doc.invoice_date.clone().unwrap_or_else(|| "-".to_string())
                                        />
                                    }
                                })
                                .collect::<Vec<_>>();

                            view! {
                                <>
                                    <Banner
                                        title="All clear — everything's handled".to_string()
                                        subtitle="Finelor has reviewed all transactions. Go run your business.".to_string()
                                        icon=LuShieldCheck
                                        tone=Tone::Success
                                        prominence=BannerProminence::Spacious
                                        trailing_icon=LuCircleCheck
                                    />

                                    <Grid columns=GridCols::Five section=true>
                                        <ActionCard href="/settings?tab=channels".to_string()>
                                            <Eyebrow>"Send documents"</Eyebrow>
                                            <CardTitle>"Channels"</CardTitle>
                                            <BodyText>"Manage Telegram, Slack, and API"</BodyText>
                                        </ActionCard>
                                        <Metric label="Total transactions" value=summary.document_counts.total.to_string() desc="synced from bank" />
                                        <Metric label="Needs review" value=summary.document_counts.pending.to_string() desc="pending approval" />
                                        <Metric label="Completed" value=summary.document_counts.completed.to_string() desc="approved or synced" />
                                        <Metric label="Completion rate" value=format!("{}%", summary.document_counts.completion_rate) desc="overall progress" />
                                    </Grid>

                                    <Grid columns=GridCols::Three section=true>
                                        <ActionCard href="/settings".to_string()>
                                            <Eyebrow>"Company information"</Eyebrow>
                                            <CardTitle>{summary.company_name.clone()}</CardTitle>
                                            <BodyText>{summary.company_org_nr.clone().unwrap_or_else(|| "No registration number yet".to_string())}</BodyText>
                                        </ActionCard>

                                        <GridItem span=GridSpan::MediumWide>
                                            <Card>
                                                <Eyebrow>"Transactions"</Eyebrow>
                                                <TextLink href="/transactions">"View all"</TextLink>
                                                <PreviewRows>{preview_rows}</PreviewRows>
                                            </Card>
                                        </GridItem>
                                    </Grid>
                                </>
                            }
                            .into_any()
                        }
                    }
                    Some(Err(err)) => view! {
                        <Alert kind=AlertKind::Error>
                            {format!("Failed to load dashboard: {}", err)}
                        </Alert>
                    }
                    .into_any(),
                    None => view! { <HiddenPlaceholder /> }.into_any(),
                }}
            </Suspense>
        </Stack>
    }
}
