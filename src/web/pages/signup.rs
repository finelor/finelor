use crate::web::components::ui::{
    Badge, BadgeStyle, Banner, BodyText, BrandTitle, Button, ButtonKind, ButtonType, Card,
    EmailInput, FeatureItem, FeatureList, FormField, Grid, GridCols, HeroTitle, LeadText,
    MarketingProofStack, PasswordInput, Space, Stack, TabLink, Tabs, TextInput, Tone,
};
use crate::web::server::auth::Signup;
use icondata::{LuArrowRight, LuFileCheck2, LuScanText, LuShieldCheck};
use leptos::prelude::*;
use leptos_icons::Icon;
use server_fn::ServerFn;

#[component]
pub fn Signup() -> impl IntoView {
    let signup_url = Signup::url();

    view! {
        <>
            <BrandTitle>"Finelor."</BrandTitle>

            <Grid columns=GridCols::SplitTwo gap=Space::Xl section=true>
                <Card>
                    <Badge tone=Tone::Info style=BadgeStyle::Soft>"Fintech-grade workflow"</Badge>
                    <div>
                        <HeroTitle>"AI bookkeeping, built for modern teams."</HeroTitle>
                        <LeadText>
                            "Upload receipts and invoices, automate categorization, and close your month with confidence in a clean, audit-friendly flow."
                        </LeadText>
                    </div>
                    <MarketingProofStack>
                        <FeatureList>
                            <FeatureItem icon=LuScanText>"Automated extraction with confidence scoring"</FeatureItem>
                            <FeatureItem icon=LuShieldCheck>"Review-first flow for uncertain fields"</FeatureItem>
                            <FeatureItem icon=LuFileCheck2>"Fast month-end close with export-ready records"</FeatureItem>
                        </FeatureList>
                        <Banner
                            title="Trusted by finance teams".to_string()
                            subtitle="Startup-ready UX with Scandinavian design principles".to_string()
                            icon=LuShieldCheck
                            tone=Tone::Success
                        />
                    </MarketingProofStack>
                </Card>

                <Card>
                    <Tabs>
                        <TabLink href="/login" active=false>"Login"</TabLink>
                        <TabLink href="/signup" active=true>"Sign Up"</TabLink>
                    </Tabs>

                    <HeroTitle>"Create your Finelor workspace"</HeroTitle>
                    <BodyText>"Create your account now. Company setup continues in the dashboard."</BodyText>

                    <form method="post" action=signup_url>
                        <Stack gap=Space::Md top=Space::Md>
                            <FormField label="Full Name">
                                <TextInput name="full_name" placeholder="Jane Doe" required=true />
                            </FormField>

                            <FormField label="Email">
                                <EmailInput name="email" placeholder="you@company.com" required=true />
                            </FormField>

                            <FormField label="Password">
                                <PasswordInput name="password" placeholder="At least 6 characters" required=true minlength="6" />
                            </FormField>

                            <FormField label="Confirm Password">
                                <PasswordInput name="confirm_password" placeholder="Repeat password" required=true />
                            </FormField>

                            <Stack top=Space::Md>
                                <Button kind=ButtonKind::Primary button_type=ButtonType::Submit full_width=true>
                                    <span>"Create account"</span>
                                    <Icon icon=LuArrowRight width="1rem" height="1rem" />
                                </Button>
                            </Stack>
                        </Stack>
                    </form>
                </Card>
            </Grid>
        </>
    }
}
