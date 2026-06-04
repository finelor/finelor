# Deploy Finelor on Hostinger

Use this guide to run Finelor on a Hostinger Docker VPS.

[![Deploy on Hostinger](https://assets.hostinger.com/vps/deploy.svg)](https://www.hostinger.com/docker-hosting?compose_url=https%3A%2F%2Fraw.githubusercontent.com%2Ffinelor%2Ffinelor%2Fmain%2Fdeploy%2Fhostinger%2Fdocker-compose.yml&REFERRALCODE=WUPFTOREF749)

The link may give eligible new Hostinger customers a 20% discount. Hostinger decides whether the discount applies at checkout.

The button starts the Hostinger setup. Hostinger may automatically load the Finelor project after checkout. If it does not, use the manual steps below.

## What You Need

Before deploying Finelor, have these ready:

- an Ollama API key. See [Environment Setup](../README.md#environment-setup);
- either a Telegram bot token. See [Telegram Setup](../README.md#telegram-setup);
- or Slack app tokens. See [Slack Setup](../README.md#slack-setup) and the [Slack app manifest](slack-app-manifest.yaml);
- optionally, your own domain name if you want to use one.

You do not need to know the Finelor hostname before creating the VPS. After the VPS is ready, use the hostname Hostinger gives you, or use your own domain if you have one.

## Compose URL

If Hostinger asks for a Docker Compose URL, paste this:

```text
https://raw.githubusercontent.com/finelor/finelor/main/deploy/hostinger/docker-compose.yml
```

## Deploy

1. Click the Deploy on Hostinger button.
2. Create a Hostinger account or sign in.
3. Choose a Docker VPS plan and complete checkout.
4. Wait for Hostinger to finish setting up the VPS.
5. Open the VPS settings in Hostinger.
6. Open Docker Manager.
7. Deploy the Finelor project if it is ready.
8. Before deploying, continue with the entering the environment values below.

### If Finelor Is Not Loaded Automatically

If Hostinger does not finish the setup automatically, finish the VPS setup yourself first:

1. Open the new VPS in Hostinger.
2. If Hostinger asks for an operating system, choose `Ubuntu 24.04 LTS`.
3. If Hostinger asks what to install or which panel to use, choose `Docker Manager`.
4. Finish the VPS setup and wait until Hostinger says it is ready.
5. Open `Docker Manager`.
6. Open `Projects`.
7. Click `Compose`.
8. Choose `Compose from URL`.
9. Paste the Compose URL from this guide.
10. Before deploying, continue with the entering the environment values below.

## Set Environment Variables

Always set:

```env
PUBLIC_HOST=<the web address you will use to open Finelor>
OLLAMA_API_KEY=<your Ollama API key>
```

For `PUBLIC_HOST`, enter the web address Hostinger gives you for Finelor, or your own domain if you connected one. Do not include `https://`.

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

## Enable Web Access to Finelor

After the Finelor project is created, deploy Hostinger Traefik from Docker Manager.

Use Hostinger's Traefik option as-is. You do not need to edit it.

Traefik lets your browser reach Finelor at:

```text
https://YOUR_FINELOR_HOSTNAME
```

## Open Finelor

After deployment, open:

```text
https://YOUR_FINELOR_HOSTNAME
```

Then:

1. Create your account.
2. Complete the setup checklist.
3. Connect Telegram or Slack.
4. Send a test invoice or receipt.

## Use Your Own Domain

You can start with a Hostinger hostname if one is available. Later, you can use your own domain.

1. Point your domain to the Hostinger VPS IP.
2. Change `PUBLIC_HOST` to your domain.
3. Redeploy Finelor.
4. Open:

```text
https://your-domain.com
```

## If Something Goes Wrong

Check these first:

- `PUBLIC_HOST` is the hostname you want to open in the browser;
- `OLLAMA_API_KEY` is set;
- `MESSAGING_PROVIDER` is set to `telegram` or `slack`;
- the token values for your chosen messaging provider are set;
- the Finelor Docker project is running.
- Traefik is deployed and running in Hostinger Docker Manager.
