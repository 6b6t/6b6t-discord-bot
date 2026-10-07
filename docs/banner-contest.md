# Banner contest operations

The feature uses the existing Discord token, MySQL databases and command-service
settings. No new deployment environment variables are required. Defaults are the
6b6t guild, announcements `1270008740707307655`, banner reviews
`1557369850593026121`, general `982192297645056040`, and Events role
`1155462541871415326`.

On 7 October 2026, startup creates tables and seeds Halloween for October and
Christmas for December. The monthly generator stays disabled until staff starts
a real contest successfully (October). Test contests and skipped rows do not
enable it. After that, it schedules next month's contest every month; November's
call is 25 October 2026 at 22:00 Europe/Warsaw. October is never generated automatically.

## After deploy

1. Wait for the `banner raw gateway ready` log. In **banner-reviews**, run
   `/bannercontest test`. It posts its call there immediately, closes submissions
   after one minute, starts voting after two minutes, and finishes after three
   minutes. The worker runs every 60 seconds, so each step can appear up to a
   minute later. Use an eligible known Minecraft username with Prime, Prime+,
   Elite, Elite+ or Apex. Upload a PNG/JPG/WEBP of at least 1280×720 and a valid email.
2. Check that the review card contains only its JPEG crop and buttons. Deny it,
   check the test notification in reviews, then approve it before voting starts.
   Also test **Other**, cancel, choose it again and enter a reason.
   Confirm modal submissions arrive and the main bot activity still reads
   "Playing IP: play.6b6t.org…" after the second gateway connects. Vote with 🔥. The result and the commands
   the bot *would* run must appear only in reviews. No DMs, general messages,
   announcement posts, guild image updates or rank grants occur in test mode.
   Repeat with an undecided entry and with no accepted entries if needed.
3. Run `/bannercontest status`. Check the test is complete. November is scheduled only after the real October call.
   Confirm `/bannerthemes list` shows October Halloween and December Christmas.
   `/bannerthemes set year:2026 month:11 theme:<theme>` sets November's theme;
   omit `theme` to clear it. Discord requires a `set` subcommand because `list`
   is a subcommand of the same group.
4. **Only after qbasty approves the first announcement and an end date**, run
   `/bannercontest start year:2026 month:10 end-date:YYYY-MM-DD`.
   This posts the October call immediately. For example, if qbasty approves
   **20 October**, run `/bannercontest start year:2026 month:10 end-date:2026-10-20`:
   submissions close 16 October at 22:00 Warsaw, voting starts 17 October at
   10:00 Warsaw, and the winner is chosen 20 October at 10:00 Warsaw. The command
   refuses an end date whose submission deadline has already passed.
5. Verify the real winner's three guild images, temporary rank and winner row.
   Live Discord permissions, file-upload UX, DMs and command-service semantics
   require this operator smoke test; local HTTP fixtures cannot prove them.

Staff commands allow Admin, Developer, or Administrator permission. Review
buttons allow Admin, Developer, Terminator and Mini Terminator roles. A decision
can change until voting starts; a denied submission cannot be replaced.
`/bannercontest skip year:YYYY month:MM` prevents that monthly contest from running.

## Persistence and manual recovery

Five feature tables are created in the existing link database without altering any
existing table: `banner_themes`, `banner_contests`, `banner_submissions`, and
`banner_winners`, plus `banner_gateway_identifies` for a persistent rolling IDENTIFY cap. No existing tables are altered. Dates are UTC Unix seconds; submission order uses Unix
milliseconds plus the submission ID as a deterministic tie breaker. Themes are
snapshotted at the call. Every accepted source is decoded with bounded memory,
centre cropped to an integral 16:9 rectangle, resized to at most 1920×1080 and
encoded once as JPEG at quality 80, under 3 MiB. Headers over 40 MP are refused
before decode; decoder allocation is capped at 160 MiB and the crop is a borrowed view. Those bytes are stored in MariaDB and reused for review,
voting, the winner post and all guild image slots. There are no expiring URL
dependencies. Email is stored only in submission/winner rows, never in bot text
or application logs. Serenity 0.12.5 silently drops username/email/file values
and resolved attachments from Label/File Upload modal submissions; it does not
reject them. Its gateway warnings remain enabled. Tungstenite frame logs are
disabled to keep raw private interaction payloads out of logs.

The Serenity `RawEventHandler` receives an already-deserialized event and cannot
recover the silently dropped values for types 18/19. A separate uncompressed JSON gateway session receives just
banner interactions (zero intents), with heartbeat, resume and reconnect support.
It waits six seconds after the main session's Ready before identifying. It
consumes one extra Discord gateway session. Fresh identifies require at least
100 remaining in Discord's budget and are capped at 20 in a rolling 24 hours,
persisted across restarts. Fatal closes 4004 and 4010–4014 stop the raw session
and report to reviews. READY/RESUMED resets reconnect backoff.
The normal Serenity session owns other commands and events.

An advisory MariaDB lock serializes contest work across instances. External
non-idempotent actions have an `effects` JSON journal in their contest row. The
attempt is persisted before network I/O. Successful posts store their message
ID. Known 429 responses retry up to three times. Contest messages, including review cards, decision notices and staff reports, use a journaled
25-character nonce with `enforce_nonce`: transient 5xx/transport errors retry
with 1, 2, 4, 8 second backoff, at most five attempts within 45 seconds.
[Discord deduplicates messages with the same nonce](https://docs.discord.com/developers/resources/message#create-message).
A restart does not repeat an uncertain message attempt, since nonce deduplication
is only guaranteed for a few minutes. Rank commands and guild image changes
are never resent. Restart resumes successful journal entries.
Safe catch-up never posts a call
after close, never opens voting with less than 12 hours remaining, and never
awards unless all vote images were public for at least 24 hours before end.
Otherwise the contest is skipped with a durable staff report and no public
catch-up post or prize. Test mode keeps its minute-long phases.

An uncertain public effect holds the phase as `paused` for manual reconciliation;
it never resends the public post or prize. Repeatable edits and reaction
PUTs/GETs retry up to five times with persisted exponential backoff
(30, 60, 120, 240 seconds). Message POSTs retry transient failures only within
the live 45-second nonce window. Definite client rejections can retry on later
ticks; transport/server failures or a restart after dispatch leave
`dispatch_started: true` and require manual reconciliation. This also prevents
duplicate general-channel pings, review cards and staff reports. An uncertain
private message affects only that effect and keeps other entries working.
Reports are stored before pausing and before delivery, including on eligibility
or identity refusal. They are flushed even for terminal contests. An uncertain
report is retained in the journal without recursive reports or blind resends.
Saved submissions get the waiting-for-review reply immediately; the worker
uploads their cards. Changed decisions and their notification snapshots are
saved together in a transaction, so rapid changes each get their own notice.
Private retries continue after public completion, skip or pause.
Metadata queries omit image blobs; bytes are fetched only
for uploads. A cleared seeded theme is stored as an empty marker, so restarts
cannot recreate it. Image/prize failures after the winner announcement report manual remediation and complete
the attempt, so the bot does not repeatedly grant or change images.

Before recovering a failed contest, inspect its journal and channel history:

```sql
SELECT id, state, effects FROM banner_contests WHERE id = <contest_id>;
```

For an uncertain message post that actually succeeded, record its real ID:

```sql
UPDATE banner_contests
SET effects = JSON_SET(effects,
    '$.call.state', 'done', '$.call.result', '<real_message_id>')
WHERE id = <contest_id>;
-- Restore the phase that stopped only after reconciling every attempted effect:
UPDATE banner_contests SET state = 'scheduled' WHERE id = <contest_id>;
```

Use the corresponding journal key and restore `open`, `review`, or `voting` for
other phases. For a delivered notification or staff report, mark its key done
with result `true`; for a review upload use the real message ID. Leave pending
`notice_` snapshots intact so the worker can acknowledge their completed
notification. The Other-menu reset holds both locks, including while another
instance finishes its worker transaction.
If an effect definitely did not happen, remove *only* that key
with `JSON_REMOVE` before restoring its phase. Never remove an uncertain prize
effect until checking the rank service: `addtemp` must never run twice.
For image failures, set the winner's `banner.jpg` in banner, splash and Discovery
splash. For prize failures, check eligibility and `/get-ranks`, run the exact
`lpv user <uuid> parent addtemp <group> 1mo` command from the review message only
if missing, then verify `/get-ranks`. LuckPerms uses an average Gregorian month
(2,629,746 seconds) for `1mo`; the logged expiry uses that duration. It is the
expected expiry from the bot's award attempt, not an authoritative LP expiry read.

Operators can also insert a scheduled row directly. Include `contest_key`
(`YYYY-MM`, unique for a real contest), `year`, `month`, `call_at`, `close_at`,
`voting_at`, and `end_at`; timestamps must be UTC Unix seconds derived from the
Warsaw calendar. `state` defaults to `scheduled`, `dry_run` to false. The worker
picks it up without a restart. Never insert October until qbasty approves.

## Local validation

```sh
cargo fmt --all --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build
# An isolated local MariaDB test database (name must start with banner_test):
BANNER_TEST_DATABASE_URL=mysql://root@127.0.0.1:33307/banner_test \
  cargo test -- --include-ignored --test-threads=1
```

The MariaDB scenario uses local HTTP stubs for Discord, image downloads and
rank/command requests. It checks schema/seed idempotence, raw forms, duplicate
constraints, decision changes, expiration, reaction pagination excluding bots,
catch-up, direct SQL rows, durable uncertain effects, DM fallback mention limits,
dry-run isolation, one three-slot guild update, prize verification and failure
reporting. A separate local websocket test covers gateway heartbeat and resume.

Floodgate Bedrock names with a leading `.` are accepted (up to 16 characters
after the dot), when `/get-ranks` resolves the name and the stats database
provides one unique UUID. If either service cannot resolve the Floodgate name,
the bot gives the unknown-username reply; no Java-name substitution is guessed.
Highest paid group is selected in order: legendultra, legend, apex, eliteultra,
elite+, elite, primeultra, prime+, prime. Thus Apex plus a previous Prime Ultra
can win Legend. Inherited Prime from VIP still counts, as the rank service returns
inherited groups. Winner eligibility and UUID are checked before any public promise.
Temporary grants go only to HTTP_PROXY_COMMAND_SERVICE; `/get-ranks` stays on
the configured rank service and verification polls for up to 30 seconds. A 403
is a definite refusal; addtemp is never resent. Public #general denial fallback
retains the exact staff reason, per qbasty's decision.

## Production-path selftest

Only members with the configured Admin or Developer role in the configured guild
may run `/bannercontest selftest`. Administrator permission alone is insufficient.
Username defaults to the dedicated `BannerSelftest` constant; every other name is
refused. The link table is checked before testing and again immediately before
grant; linked accounts refuse. The authenticated proxy `/players` list must
prove the account offline by UUID and name at both checks. An unavailable or
malformed list refuses the test. `/get-ranks` currently supplies no
online-presence field; rank presence alone cannot prove offline status. Keep
this account offline and unlinked throughout the test. Groups are primeultra
(default) or eliteultra; legend is refused. Existing and inherited groups refuse
the test.
The stats UUID must match the UUID returned by `/get-ranks` on every check. A
service response without UUID identity proof refuses the rank test; the bot does
not change the service API or assume username uniqueness in LuckPerms.

Allow only `lpv user <uuid> parent addtemp <group> 1m` and
`lpv user <uuid> parent removetemp <group> 1m` for this test. Removal subtracts
only its minute and runs only after verified presence. Otherwise the bot waits
until 70 seconds after dispatch and checks absence. Never blindly repeat grants.
Proxy routing uses HTTP_PROXY_COMMAND_SERVICE_BASE_URL; rank lookup uses
HTTP_SLAVE1_COMMAND_SERVICE_BASE_URL with the configured fallback. Acceptance
means dispatched, not completed execution.

Reports go to banner-reviews. The image step only GETs the current guild and
CDN PNG representations, then runs each available slot through the shared winner
encoder offline. It reports JPEG byte size, dimensions, the documented PNG/JPEG
and 16:9 requirements, the banner minimum of 960x540, and the encoder's
conservative <3 MiB bound. Missing images skip; download or encoder failures
report failure while rank and DM tests continue. No guild image PATCH, backup upload, restoration or recovery obligation
is created. Animated banners are checked using their static PNG representation.
Discord CDN bytes cannot prove original upload bytes. Feature eligibility and
Discord HTTP acceptance are not tested by this offline check.
See [Discord Modify Guild](https://github.com/discord/discord-api-docs/blob/main/developers/resources/guild.mdx#modify-guild).
See [Discord Server Banners](https://support.discord.com/hc/en-us/articles/360028716472-Server-Banners)
for the banner dimensions. The <3 MiB bound is a local safety limit, not a
documented Discord byte-size guarantee.
The staff decision-notice DM uses banner-reviews fallback, never general or announcements.

Rank presence and absence use the same verifier as real winners, with UUID
validation for selftest and a 30-second elapsed deadline including slow requests.
Inspect the independent results and final summary. Run outside active contests
and first-of-month Warsaw 09:00–10:59; the test holds contest locks.

Operational rows are skipped, dry-run audit rows with year/month 2026/10 and
selftest-UUID or gateway-stop keys; status excludes them. Gateway persistence
retries every 60 seconds without a cap. Discord delivery remains bounded,
nonce-journaled and held when uncertain. READY/RESUMED archives the previous
stop event automatically, so a later stop alerts again. Preserve uncertain
history and reconcile channel receipts; do not delete keys to re-arm alerts.

Banner schema failure disables only the banner service; linked accounts, rank
synchronization and other database features retain their pools.
