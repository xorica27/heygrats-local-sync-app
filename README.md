# HeyGrats Local Sync

HeyGrats Local Sync helps photobooth operators send new files from a local booth folder to a HeyGrats event wall in real time.

## Download

1. Open the latest release:  
   https://github.com/xorica27/heygrats-local-sync-app/releases/latest
2. Download the installer for your device:
   - macOS: `.dmg`
   - Windows: `.msi` or `.exe` (when available in the release assets)

## Install

### macOS

1. Open the downloaded `.dmg`.
2. Drag **HeyGrats Local Sync** into **Applications**.
3. Open the app.

If macOS blocks first launch:

- Go to **System Settings → Privacy & Security**.
- Under Security, click **Open Anyway** for HeyGrats Local Sync.
- Open the app again.

### Windows

1. Run the downloaded installer.
2. Follow the install steps.
3. Open **HeyGrats Local Sync** from Start Menu/Desktop.

If SmartScreen warns on first launch, choose **More info → Run anyway** only if the file was downloaded from the official release page above.

## First-time setup

Before starting sync, prepare:

- A Sync Token from **heygrats.com → Dashboard/Local Sync**
- The local folder where your booth saves photos

Then in the app:

1. Paste your Sync Token.
2. Select your watched folder.
3. Click **Start Sync**.

The app connects to `https://heygrats.com` by default.

## What the app does

- Watches your local folder for new media.
- Uploads supported media to your HeyGrats event wall.
- Shows sync activity logs.
- Lets you clear local sync cache after an event.

## Security and permissions

- The app needs file access to the folder you choose for syncing.
- The app sends media and sync requests to HeyGrats services.
- Do not share your Sync Token with others.
- Use installers only from the official GitHub Releases page.

## Supported media

- Images only: `jpg`, `jpeg`, `png`, `webp`, `avif`, `heic`, `heif`
- Not supported: `gif`, video files, and all other non-image formats
