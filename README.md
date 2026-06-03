# Finelor - Accounting Department on Autopilot

<div align="center">

> “The future of accounting is not software humans operate — it is autonomous AI systems businesses collaborate with.”

</div>

We are building and sharing openly the foundational infrastructure for autonomous and intelligent, AI-native financial operations where businesses interact conversationally with intelligent agents instead of traditional accounting systems and manual workflows.

## Deploy on Hostinger

[![Deploy on Hostinger](https://assets.hostinger.com/vps/deploy.svg)](https://www.hostinger.com/docker-hosting?compose_url=https%3A%2F%2Fraw.githubusercontent.com%2Ffinelor%2Ffinelor%2Fmain%2Fdeploy%2Fhostinger%2Fdocker-compose.yml&REFERRALCODE=WUPFTOREF749)

Start the Hostinger Docker VPS setup for Finelor from the prebuilt `ghcr.io/finelor/finelor:latest` image. The referral link may provide a 20% Hostinger discount for eligible new customers. See the [Hostinger deployment guide](docs/hostinger.md) for the remaining Hostinger setup steps, required values, first login, and domain setup.

## Features

- document (invoice and receipts) ingestion and processing
- accounting verification generation
- bookkeeping assistance
- reconciliation workflows
- approval workflows
- export to accounting systems
- human-in-the-loop clarifications
- intelligent conversational support through Telegram and Slack
- web-based control panel app
- and many more coming soon

## Install & Run

### Prerequisites

- Rust toolchain (see `rust-toolchain.toml`)
- Docker + Docker Compose (for Docker mode)

### Environment Setup

1. Copy environment template:

    ```bash
    cp .env.example .env
    ```

2. Add your Ollama API key to `.env` (get your free account on [`Ollama website`](https://ollama.com/)).

3. Choose one messaging provider (`telegram` or `slack`) in `.env`.

4. Add the matching provider credentials. See Telegram Setup or Slack Setup.

### Telegram Setup

Create a Telegram bot via BotFather.

1. In Telegram search for **@BotFather**, or in your browser visit [`t.me/BotFather`](t.me/BotFather)
2. In **@BotFather** chat click `Open` or send `/newbot`.
3. Give your bot a display name (e.g., "Finelor").
4. Choose a username that ends in `bot`. It must be unique (e.g., `my_finelor_bot`).
5. BotFather gives you an API token.
6. Set `MESSAGING_PROVIDER` to `telegram`.
7. Add the bot token to `MESSAGING_TELEGRAM_BOT_TOKEN` in `.env`.

### Slack Setup

Create a Slack app via the Slack app manifest.

1. Open [Slack apps](https://api.slack.com/apps).
2. Click `Create New App`.
3. Choose `From an app manifest`.
4. Select your Slack workspace.
5. Copy the contents of [`docs/slack-app-manifest.yaml`](docs/slack-app-manifest.yaml) into the **YAML** tab in Slack. See Slack's [app manifest docs](https://docs.slack.dev/app-manifests) for details.
6. Click through Slack's review steps and create the app.
7. In the app settings, open **OAuth & Permissions** and click `Install to <YOUR WORKSPACE NAME>`.
8. Click `Allow` on the opened authorization page.
9. Copy the **Bot User OAuth Token**. It starts with `xoxb-`.
10. Open **Basic Information > App-Level Tokens** and generate a token with the [`connections:write`](https://docs.slack.dev/reference/scopes/connections.write/) scope. It starts with `xapp-`.
11. Confirm [Socket Mode](https://api.slack.com/apis/connections/socket) is enabled for the app in **App Settings > Socket Mode**.
12. Set `MESSAGING_PROVIDER` to `slack` in `.env`.
13. Add the `xoxb-` token to `MESSAGING_SLACK_BOT_TOKEN` in `.env`.
14. Add the `xapp-` token to `MESSAGING_SLACK_APP_TOKEN` in `.env`.

### Option A (Recommended): Run in Docker (everything in Docker)

Development image/stack:

```bash
make dev-up
```

Release-style image/stack:

```bash
make release-up
```

Stop and clean volumes:

```bash
make dev-clean
# or
make release-clean
```

Stop without cleaning:

```bash
make dev-down
# or
make release-down
```

Health check:

```bash
make health
```

### Option B: Run on Host (everything on host)

1. Install local toolchain dependencies:

```bash
make bootstrap-local
```

1. Run app on host:

```bash
make run
```

## First Time Use

1. Sign up on `http://localhost:3000`
2. Visit your profile page
3. In **Channels** page connect your telegram to your Finelor bot.
4. Send your first invoice/receipt to Finelor bot.

## Advanced Configuration (Quick)

- `config.yaml` is the required base application configuration.
- `config.local.yaml` is an optional local override file (best for host/local development).
- `.env` and shell environment variables provide values used by config placeholders and final runtime overrides.

Override order (lowest -> highest):

1. `config.yaml`
2. `config.local.yaml` (if present)
3. environment variables (shell-exported and/or loaded from `.env`)

Notes:

- `config.yaml` must exist.
- `config.local.yaml` can be partial (only include keys you want to override).
- `config.local.yaml` is for host/local runs and is not used in Docker by default.
- for the same key, shell-exported env values override `.env` values.

## Documentation

Project documentation lives in [`docs/`](docs/).

## Contribution

We welcome contributions.

1. Fork the repository and create a feature branch.
2. Make your changes with focused commits.
3. Run baseline checks before opening a PR:

```bash
make check
make docs-check
```

1. For DB-backed integration checks:

```bash
make test-integration
```

1. Open a pull request with a clear summary, test evidence, and any docs updates.

## Community

Join the Discord community:

- Discord: [``https://discord.gg/Fvydsb8j8x``](https://discord.gg/Fvydsb8j8x)

## License

Apache 2.0 — see [`LICENSE`](LICENSE).

Built by [`Finelor Team`](https://finelor.com).
