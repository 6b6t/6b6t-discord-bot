# Banner contest operations

The feature uses the existing Discord token, MySQL databases and command-service
settings. No new deployment environment variables are required. Defaults are the
6b6t guild, announcements `1270008740707307655`, banner reviews
`1557369850593026121`, general `982192297645056040`, and Events role
`1155462541871415326`.

On 7 October 2026, startup creates tables and seeds Halloween for October and
Christmas for December. It creates November's scheduled row silently. Its call
is 25 October 2026 at 22:00 Europe/Warsaw. October is never generated automatically.

## After deploy

1. Wait for the `banner raw gateway ready` log. In **banner-reviews**, run
   `/bannercontest test`. It posts its call there immediately, closes submissions
   after one minute, starts voting after two minutes, and finishes after three
   minutes. The worker runs every 60 seconds, so each step can appear up to a
   minute later. Use an eligible known Minecraft username with Prime, Prime+,
   Elite, Elite+ or Apex. Upload a PNG/JPG/WEBP of at least 1280×720 and a valid email.
2. Check that the review card contains only its JPEG crop and buttons. Deny it,
   check the test notification in reviews, then approve it before voting starts.
   Also test **Other** and enter a reason. Vote with 🔥. The result and the commands
   the bot *would* run must appear only in reviews. No DMs, general messages,
   announcement posts, guild image updates or rank grants occur in test mode.
   Repeat with an undecided entry and with no accepted entries if needed.
3. Run `/bannercontest status`. Check the test is complete and November is scheduled.
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

Four new tables are created in the existing link database without altering any
existing table: `banner_themes`, `banner_contests`, `banner_submissions`, and
`banner_winners`. Dates are UTC Unix seconds; submission order uses Unix
milliseconds plus the submission ID as a deterministic tie breaker. Themes are
snapshotted at the call. Every accepted source is decoded with bounded memory,
centre cropped to an integral 16:9 rectangle, resized to at most 1920×1080 and
encoded once as JPEG. Those bytes are stored in MariaDB and reused for review,
voting, the winner post and all guild image slots. There are no expiring URL
dependencies. Email is stored only in submission/winner rows, never in bot text
or application logs. Serenity's payload-error logging is disabled because its
older decoder would otherwise print the private raw file-upload modal.

The Serenity `RawEventHandler` receives an already-deserialized event and cannot
handle types 18/19. A separate uncompressed JSON gateway session receives just
banner interactions (zero intents), with heartbeat, resume and reconnect support.
It waits six seconds after the main session's Ready before identifying. It
consumes one extra Discord gateway session; do not run duplicate deployments.
The normal Serenity session owns other commands and events.

An advisory MariaDB lock serializes contest work across instances. External
non-idempotent actions have an `effects` JSON journal in their contest row. The
attempt is persisted before network I/O. Successful posts store their message
ID. Known 429 responses retry up to three times; transport timeouts and other
failures do not retry a post or rank command. Restart resumes successful journal
entries and catches up every due phase. An uncertain effect stops the contest
as `failed` and reports the specific manual action in reviews. Image/prize
failures after the winner announcement report manual remediation and complete
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
other phases. If an effect definitely did not happen, remove *only* that key
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
  cargo test -- --include-ignored
```

The MariaDB scenario uses local HTTP stubs for Discord, image downloads and
rank/command requests. It checks schema/seed idempotence, raw forms, duplicate
constraints, decision changes, expiration, reaction pagination excluding bots,
catch-up, direct SQL rows, durable uncertain effects, DM fallback mention limits,
dry-run isolation, one three-slot guild update, prize verification and failure
reporting. A separate local websocket test covers gateway heartbeat and resume.
