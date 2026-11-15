# Clock & calendar

With **Settings > Clock & calendar > Opens from the taskbar clock** on, a click on the clock (or
Win+Alt+D) opens Audio Tray's calendar instead of Windows'. Audio Tray's low-level mouse hook
swallows the click, because the shell's clock button opens its own flyout whatever its XAML
handlers do with the press. It shows two cards:

- **Windows' notifications**, grouped by app, above. Each shows the sender's photo, its text and
  any large picture, with the app's own buttons: **Reply** opens a box under it (Enter sends,
  Escape cancels), and ⋯ holds the rest (mute, turn off). Clicking a notification does what a
  click does in Windows' centre. The pictures and buttons come from Windows' own notification
  store (`wpndatabase.db`, read-only), since the notification listener gives only the text; the
  buttons and replies reach the app through the COM class it registers for them
  (`ToastActivatorCLSID`). An app without one is only opened. The card folds to one line when the
  calendar needs the room.
- **The calendar**, below: today's date and Windows' additional clocks (Date and time settings >
  Additional clocks), a week strip that opens into the month (with optional ISO week numbers),
  the next event, the rest of the week, and Focus / Do not disturb / Settings at the foot.
  **Join** works for Teams, Meet, Zoom and Webex meetings. An event opens into its details: time,
  place, organiser and your reply, who's invited, notes, and buttons to join or open it in its
  calendar.
- **The focus timer** starts a Windows focus session, so Windows' Do not disturb runs with it, and
  the clock shows the time left.

It opens from whichever screen edge the taskbar is on, over Windows' acrylic, sliding in and out
like the shell's own flyouts.

## The taskbar clock

The TAP (in Explorer) writes the clock's two lines itself once anything differs from Windows' own
format, formatting the time with the locale's patterns:

- seconds, the day of the week, one line, or no date (`time_pattern` and `date_pattern` under
  `[clock]` in config.toml take your own `GetTimeFormatEx`/`GetDateFormatEx` patterns);
- the focus time left after the time, while a focus timer runs (meetings stay off the clock);
- an accent-coloured dot and the unread count before the time.

Once it has written the clock, the TAP keeps it ticking for the rest of Explorer's life, going back
to the locale's own format when Audio Tray quits: the shell might not take its clock back on its
own.

## Meetings and focus

- **Reminders** on the edge pill, 1, 5 or 10 minutes before an event: a banner you click to join.
- **Do not disturb** (a Windows focus session) during calendar events, during calls (Teams,
  Discord, or a meeting app holding the microphone), or both. Audio Tray only ends the sessions it
  started.
- Events are fetched in the background every 15 minutes while there are sources, for the
  reminders and the clock. A tray timer wakes only for the next thing due.

## Calendars

Once fetched, every calendar is listed in Settings with a switch to hide it and colours to pick
from (its own first). Hidden calendars send no reminders.

## Sources

Events come from up to three kinds of source, in any mix:

| Source | How | What it needs |
| --- | --- | --- |
| A Google account | Google Calendar API, read-only | Your own OAuth client (below) |
| A Microsoft account (Outlook, work/school or personal) | Microsoft Graph, read-only | Your own app registration (below) |
| Calendar links | Any iCal/ICS address | Nothing: paste the address |

Sign-ins happen in your browser. Audio Tray keeps only each account's refresh token, in the
Windows Credential Manager (`AudioTray/calendar-google`, `AudioTray/calendar-microsoft`), along
with the Google client secret and the calendar links. None of these go in `config.toml`.

Events are fetched when the calendar opens (cached ones show at once, refreshed when more than 5
minutes old), when you move to a month outside the three already fetched, and every 15 minutes in
the background for reminders and the clock.

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
