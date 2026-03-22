# HeyGrats Local Sync App

Desktop sync companion for HeyGrats photobooth workflows.

This app watches a local export folder on the booth machine and uploads new
photos or videos to a HeyGrats event wall in near real time.

## What it does

- Connects to a HeyGrats site using an event sync token
- Watches a local folder for new media
- Uploads supported files directly into the HeyGrats local-sync pipeline
- Prompts for local cleanup after a sync session is stopped

## Stack

- Tauri 2
- Vite
- Rust

## Requirements

- Node.js 20+
- Rust stable toolchain
- A running HeyGrats deployment with the local-sync backend endpoints enabled

## Development

Install dependencies:

```bash
npm install
```

Run in development:

```bash
npm run dev
```

## Production Build

```bash
npm run build
```

Tauri will create platform-specific app bundles in `src-tauri/target/release/bundle/`.

## Operator Inputs

The booth operator needs:

- A sync token generated from `Dashboard -> Local Sync`
- The local photobooth export folder

By default, the app connects to `https://heygrats.com`.

If you are testing against local or staging environments, open the `Advanced`
section and provide an origin override there.

## GitHub Releases

This repo includes a GitHub Actions workflow that builds macOS and Windows
installers automatically whenever a GitHub Release is published.

## Supported Media

- Images: `jpg`, `jpeg`, `png`, `gif`, `webp`, `avif`, `heic`, `heif`
- Video: `mp4`, `mov`, `webm`
