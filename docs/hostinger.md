# Deploy Finelor on Hostinger

Use this guide to run Finelor on a Hostinger Docker VPS.

[![Deploy on Hostinger](https://assets.hostinger.com/vps/deploy.svg)](https://www.hostinger.com/docker-hosting?compose_url=https%3A%2F%2Fraw.githubusercontent.com%2Ffinelor%2Ffinelor%2Fmain%2Fdeploy%2Fhostinger%2Fdocker-compose.yml&REFERRALCODE=WUPFTOREF749)

The link may give eligible new Hostinger customers a 20% discount. Hostinger decides whether the discount applies at checkout.

## What You Need

Before deploying, have these ready:

- an Ollama API key. See [Environment Setup](../README.md#environment-setup);
- either a Telegram bot token. See [Telegram Setup](../README.md#telegram-setup);
- or Slack app tokens. See [Slack Setup](../README.md#slack-setup) and the [Slack app manifest](slack-app-manifest.yaml).

You do not need to know the server IP before you start. Hostinger gives you the IP after the VPS is created.

## Deploy

1. Click the Deploy on Hostinger button.
2. Create a Hostinger account or sign in.
3. Choose a Docker VPS plan and complete checkout.
4. Wait for Hostinger to create the VPS.
5. Copy the VPS public IP from Hostinger.
6. In the Docker project setup, set `PUBLIC_HOST` to that public IP.
7. Add your Ollama API key.
8. Choose Telegram or Slack and add the matching token values.
9. Deploy the project.

## Values to Enter

Always set:

```env
PUBLIC_HOST=<your VPS public IP>
OLLAMA_API_KEY=<your Ollama API key>
```

For Telegram, set:

```env
MESSAGING_PROVIDER=telegram
MESSAGING_TELEGRAM_BOT_TOKEN=<your Telegram bot token>
```

For Slack, set:

```env
MESSAGING_PROVIDER=slack
MESSAGING_SLACK_BOT_TOKEN=<your Slack bot token>
MESSAGING_SLACK_APP_TOKEN=<your Slack app token>
```

Use either Telegram or Slack, not both.

## Open Finelor

After deployment, open:

```text
https://YOUR_SERVER_IP
```

Your browser may show a certificate warning when using the server IP. This is expected for the first test deployment. Continue only if the IP matches your Hostinger VPS.

Then:

1. Create your account.
2. Complete the setup checklist.
3. Connect Telegram or Slack.
4. Send a test invoice or receipt.

## Use a Domain Later

For a cleaner setup, use a domain after the first test works.

1. Point your domain to the Hostinger VPS IP with an `A` record.
2. In the Docker project settings, change `PUBLIC_HOST` from the IP to your domain.
3. Redeploy the project.
4. Open:

```text
https://your-domain.com
```

Finelor uses Caddy in front of the app. With a real domain, Caddy should automatically set up a trusted HTTPS certificate.

## If Something Goes Wrong

Check these first:

- `PUBLIC_HOST` is set to the VPS IP or your domain;
- `OLLAMA_API_KEY` is set;
- `MESSAGING_PROVIDER` is set to `telegram` or `slack`;
- the token values for your chosen messaging provider are set;
- the Docker project is running in Hostinger.

The image used by Hostinger is:

```text
ghcr.io/finelor/finelor:latest
```
