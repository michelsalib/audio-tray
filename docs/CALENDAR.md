# Calendar

With **Settings > Connectors > Calendar** on, a click on the taskbar clock opens Audio Tray's
calendar instead of Windows': the month, a dot on each day that has events, and the selected day's
agenda with a **Join** button for Teams and Meet meetings. Windows' notification centre stays on
the bell beside the clock, on the calendar's own bell, and on Win+N.

Events come from up to three kinds of source, in any mix:

| Source | How | What it needs |
| --- | --- | --- |
| A Google account | Google Calendar API, read-only | Your own OAuth client (below) |
| A Microsoft account (Outlook, work/school or personal) | Microsoft Graph, read-only | Your own app registration (below) |
| Calendar links | Any iCal/ICS address | Nothing: paste the address |

Sign-ins happen in your browser. Audio Tray keeps only each account's refresh token, in the
Windows Credential Manager (`AudioTray/calendar-google`, `AudioTray/calendar-microsoft`), along
with the Google client secret and the calendar links. None of these go in `config.toml`.

Nothing polls. Events are fetched when the calendar opens: cached ones show at once, and they are
refreshed if they are more than 5 minutes old. They are fetched again when you move to a month
outside the three already fetched.

## Google account

Audio Tray signs in through an OAuth client of your own, as Discord's integration does.

1. Open the [Google Calendar API](https://console.cloud.google.com/apis/library/calendar-json.googleapis.com)
   page (Settings' first **Open**), create a project if asked, and click **Enable**.
2. Set up **OAuth consent screen**: *External*, an app name, your address. Then click
   **Publish app** under *Audience*. While the app is in *Testing*, Google ends its sign-ins after
   7 days. A published but unverified app works for you; Google shows an "unverified app" warning
   at sign-in, which you continue past.
3. **Credentials > Create credentials > OAuth client ID**, type **Desktop app** (Settings' second
   **Open**). Copy the client ID and press its **Paste**, then do the same for the client secret.
4. Press **Sign in**. The browser asks you to choose the account and allow read access to your
   calendars.

The calendars shown are those ticked in Google Calendar's own list.

## Microsoft account (Outlook)

1. Open **App registrations** in the [Entra admin center](https://entra.microsoft.com/) (Settings'
   **Open**) and choose **New registration**. For *Supported account types*, pick **Accounts in any
   organizational directory and personal Microsoft accounts**.
2. **Authentication > Add a platform > Mobile and desktop applications**, and add the redirect
   `http://localhost` (Settings' **Copy** puts it on the clipboard).
3. **API permissions > Add a permission > Microsoft Graph > Delegated**: `Calendars.Read`
   (`User.Read` is there already).
4. Copy the **Application (client) ID** from the Overview page and press its **Paste**, then
   **Sign in**.

A company account can refuse an app it has not approved ("Your organisation must approve the app
first"). An administrator then has to grant consent, or you use a calendar link instead.

If your company does not allow registering apps, register it in a tenant of your own (a free Azure
account), using the any-organisation account type from step 1. It can still sign in to your work
account, provided your company allows users to consent to apps.

For a single-tenant registration, set `microsoft_tenant` in `[calendar]` (config.toml) to the
tenant's ID.

## Calendar links

Copy a calendar's secret iCal address, then press **Paste link**:

- **Google:** the calendar's *Settings and sharing > Integrate calendar > Secret address in iCal
  format*.
- **Outlook on the web:** *Settings > Calendar > Shared calendars > Publish a calendar*, *Can view
  all details*, then the ICS link. Companies often turn publishing off.

Recurring events are expanded by Audio Tray. Times named with a time zone (`TZID`) are read on this
PC's clock, which is right for a calendar kept in your own time zone.
