# Full Refactor of hkmu-ole-attendance — Design

**Date:** 2026-08-29
**Status:** Approved

## Goal

Restructure the single 635-line `main.py` into focused modules, remove the
abandoned `selenium-wire` dependency (source of the `blinker` and `pyOpenSSL`
version pins), fix the retry recursion between `send_classes` and
`daily_attendance_task`, replace `print` with `logging`, and honor the
`SCHEDULE_TIME` environment variable. All other runtime behavior is preserved.

Out of scope (explicitly declined): pytest test suite, type hints, replacing
the `threading.Timer` concurrency model, any change to Discord message
formats or the attendance polling semantics.

## Module layout (flat files at repo root)

| File | Responsibility |
|---|---|
| `config.py` | Env loading via python-dotenv: `OLE_URL`, `STUDENT_ID`, `STUDENT_PASSWORD`, `DISCORD_WEBHOOK`, `SCHEDULE_TIME` (default `03:00`). Hong Kong timezone object and `get_hk_time()`. Shared constants: WebDriver wait timeouts, attendance poll interval (600 s), post-refresh waits, fetch/login retry counts and delays, class endtime fallback (+3 h), warning threshold (30 min). |
| `notify.py` | `send_discord_notification(message)` — the single webhook sender. When `DISCORD_WEBHOOK` is unset, logs the message and returns `True` (current behavior). |
| `ole.py` | `create_driver()` — plain Selenium Firefox (headless options preserved, selenium-wire options removed, fallback init removed since it only existed for wire failures). `login(driver, username, password)` — current `enter_ole` flow: navigate, wait for `userid`, submit credentials, wait for redirect. `fetch_today_classes(driver)` — see fetch design below. |
| `classes.py` | Pure logic, no I/O or Selenium: `parse_class_time(datetime_str)`, `filter_today_classes(json_data)`, `build_class_infos(classes)` (produces the scheduling dicts incl. the per-class course URL), `format_classes_message(classes)` (builds the Discord "Retrieved Classes" text, or the no-classes / failure text). |
| `attendance.py` | `schedule_attendance(class_infos, username, password)` — Timer scheduling, in-progress immediate start, already-ended skip. `mark_attendance(class_info, username, password)` — logs in with a fresh driver, loads the class URL, polls for `submitted_msg` every 10 minutes until endtime, sends the 30-minute warning once, sends success/failure/error notifications, quits the driver in `finally`. |
| `main.py` | `daily_attendance_task()` orchestration and the `schedule` main loop. |

Rationale for `notify.py` (not `discord.py`): a local `discord.py` would shadow
the `discord` PyPI package if it were ever installed.

## Class-list fetch (selenium-wire replacement)

Current behavior passively sniffs the page's XHR to
`https://oleconnect.hkmu.edu.hk/oledb/api/getTodayClass/` and needs a blind
30-second sleep to let it happen. The API is on a different domain than the
login site (`iole.hkmu.edu.hk`), so driver-cookie extraction into a
`requests.Session` is unreliable.

**New primary path:** after login redirect, `fetch_today_classes(driver)` runs
`driver.execute_async_script` executing
`fetch('https://oleconnect.hkmu.edu.hk/oledb/api/getTodayClass/', {credentials: 'include'})`
inside the logged-in page and returns the parsed JSON. This runs in the same
origin/session context as the page's own XHR, so cookies and CORS behave
identically.

**Fallback path:** if the in-page fetch throws or returns non-JSON,
`driver.get(API_URL)` and parse `document.body.textContent` as JSON (covers
the cookie-authenticated case).

**Retry (fetch-level, inside `ole.py`):** within one logged-in driver
session, the fetch (primary + fallback) is attempted up to 3 times with a
10 s delay between attempts, replacing the old 30 s sleep. This is distinct
from — and nested inside — the orchestration-level retry described in
"Retry-flow fix" below, which re-does login+fetch with a fresh driver.

**Revert path:** all fetch logic is isolated in `ole.py`; if the live site
rejects both paths, reverting to selenium-wire touches one file plus
requirements.

## Retry-flow fix

Today, `send_classes` recursively calls `daily_attendance_task(retries + 1)`
when the fetch result is bad, while the original invocation continues on to
`schedule_attendance` — a layering violation with a latent double-scheduling
path.

New flow in `daily_attendance_task`:

1. Login (existing 3-attempt loop, 10 s apart — unchanged).
2. Fetch today's classes; on bad result (`result != 1` or `None`), retry the
   whole login+fetch up to 3 times, 30 s apart (matching today's intent).
3. Quit the fetch driver.
4. Send the classes notification **once** with the final result (success list,
   "No classes scheduled for today!", or "Failed to retrieve class
   information").
5. Schedule attendance **once** (skipped if fetch ultimately failed).

`notify.py` and `classes.py` never call back into the orchestration layer.

## Logging

Replace all `print` calls with the `logging` module: `basicConfig` to stdout
with timestamp + level, module-level loggers. Docker log visibility is
unchanged (`PYTHONUNBUFFERED=1` already set).

## Configuration change

`SCHEDULE_TIME` (documented in `.env.example` but previously ignored) is read
in `config.py` with default `03:00` and passed to
`schedule.every().day.at(...)`.

## Dependency and Docker changes

- `requirements.txt`: remove `selenium-wire`, remove the `blinker==1.6.3` pin,
  remove the `pyOpenSSL<26.2` pin; add `selenium>=4`. Keep `requests`
  (Discord webhook), `schedule`, `python-dotenv`, `pytz`.
- `Dockerfile`: change `COPY main.py .` to `COPY *.py .`. No other changes.
- `README.md`: no content changes required (behavior is identical from the
  user's perspective).

## Error handling

All existing notification paths preserved: configuration-error message when
credentials are missing, daily-setup error message on unexpected exceptions,
attendance success / 30-minute warning / failure / error messages unchanged
in format and trigger conditions.

## Verification

Login only works against the live university system, so:

1. Static: `python -m py_compile` on all modules; import smoke test.
2. Manual live run by the user: start the container (startup immediately runs
   `daily_attendance_task`), confirm the class list arrives and the Discord
   message matches the old format.
3. Known risk, accepted: if the API requires a JS-generated request header,
   the in-page fetch still succeeds (it runs inside the page that owns the
   session); the navigation fallback covers cookie-only auth. If both fail,
   use the revert path.
