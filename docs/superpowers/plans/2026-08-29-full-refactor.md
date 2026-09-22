# Full Refactor Implementation Plan

> **Historical.** This describes the earlier **Python** implementation
> (`main.py`, Selenium/`selenium-wire`) and is kept for reference only. The
> project has since been rewritten in Rust; see [`README.md`](../../../README.md)
> for the current architecture.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split the 635-line `main.py` into six focused modules, remove the abandoned `selenium-wire` dependency (replacing XHR sniffing with an in-page `fetch`), fix the `send_classes`→`daily_attendance_task` retry recursion, replace `print` with `logging`, and honor the `SCHEDULE_TIME` env var.

**Architecture:** Flat modules at repo root: `config.py` (env + constants), `notify.py` (Discord), `classes.py` (pure data logic), `ole.py` (Selenium driver/login/fetch), `attendance.py` (scheduling + marking loop), `main.py` (orchestration). The class-list API is called actively from inside the logged-in page via `driver.execute_async_script(fetch(...))` instead of passively sniffed.

**Tech Stack:** Python 3.11 (Docker) / 3.12 (local), Selenium 4 + Firefox/geckodriver, `requests`, `schedule`, `python-dotenv`, `pytz`.

**Spec:** `docs/superpowers/specs/2026-08-29-full-refactor-design.md`

## Global Constraints

- No pytest suite, no type hints (explicitly declined by user). Verification is `py_compile`, import smoke checks, and inline `python -c` behavior probes.
- Discord message formats must stay **byte-identical** to the old code (headers like `**- Attendance Confirmed!**`, `**- ATTENDANCE WARNING**`, `**- Attendance FAILED**`, `**- Attendance ERROR**`, `**Configuration Error**`, `**Daily Setup Error**`, `**Retrieved Classes:**`).
- Attendance semantics unchanged: poll every 600 s, one 30-minute warning, endtime fallback = start + 3 h, `threading.Timer` daemon threads.
- Dependencies after refactor: exactly `selenium`, `requests`, `schedule`, `python-dotenv`, `pytz`. No `selenium-wire`, no `blinker` pin, no `pyOpenSSL` pin.
- All local verification commands use `.venv/bin/python` (created in Task 1).
- The old `main.py` stays untouched until Task 6; every commit in between must leave the repo in a state where `python -m py_compile` passes on all `.py` files.

---

### Task 1: Local venv + `config.py`

**Files:**
- Create: `config.py`

**Interfaces:**
- Consumes: nothing (leaf module).
- Produces (used by every later task):
  - Settings: `OLE_URL`, `DISCORD_WEBHOOK`, `STUDENT_ID`, `STUDENT_PASSWORD`, `SCHEDULE_TIME` (all `str`), `TODAY_CLASS_API_URL: str`, `HONG_KONG_TZ` (pytz tzinfo)
  - Constants (all `int`): `LOGIN_PAGE_TIMEOUT=15`, `LOGIN_REDIRECT_TIMEOUT=10`, `PAGE_LOAD_WAIT=15`, `ELEMENT_CHECK_WAIT=3`, `POST_REFRESH_WAIT=10`, `LOGIN_MAX_RETRIES=3`, `LOGIN_RETRY_DELAY=10`, `FETCH_MAX_ATTEMPTS=3`, `FETCH_RETRY_DELAY=10`, `DAILY_TASK_MAX_RETRIES=3`, `DAILY_TASK_RETRY_DELAY=30`, `ATTENDANCE_POLL_INTERVAL=600`, `WARNING_THRESHOLD_MINUTES=30`, `DEFAULT_CLASS_DURATION_HOURS=3`
  - `get_hk_time() -> datetime` (tz-aware, Asia/Hong_Kong)

- [ ] **Step 1: Create the venv with the post-refactor dependency set**

```bash
cd /home/avan/workspace/hkmu-ole-attendance
python3 -m venv .venv
.venv/bin/pip install "selenium>=4" "requests>=2.32.5" "schedule>=1.2.2" "python-dotenv>=1.1.1" "pytz>=2025.2"
```

Expected: pip installs succeed. (`.venv` is already in `.gitignore` — do not commit it.)

- [ ] **Step 2: Write `config.py`**

```python
import os
from datetime import datetime

import pytz
from dotenv import load_dotenv

# Load environment variables from .env file
load_dotenv()

# Configuration from environment variables
OLE_URL = os.getenv("OLE_URL", "https://iole.hkmu.edu.hk")
DISCORD_WEBHOOK = os.getenv("DISCORD_WEBHOOK", "")
STUDENT_ID = os.getenv("STUDENT_ID", "")
STUDENT_PASSWORD = os.getenv("STUDENT_PASSWORD", "")
SCHEDULE_TIME = os.getenv("SCHEDULE_TIME", "03:00")

# The API endpoint the OLE dashboard's own XHR calls for today's classes
TODAY_CLASS_API_URL = "https://oleconnect.hkmu.edu.hk/oledb/api/getTodayClass/"

# Timezone configuration
HONG_KONG_TZ = pytz.timezone("Asia/Hong_Kong")

# WebDriver waits (seconds)
LOGIN_PAGE_TIMEOUT = 15
LOGIN_REDIRECT_TIMEOUT = 10
PAGE_LOAD_WAIT = 15
ELEMENT_CHECK_WAIT = 3
POST_REFRESH_WAIT = 10

# Retry and polling behavior
LOGIN_MAX_RETRIES = 3
LOGIN_RETRY_DELAY = 10
FETCH_MAX_ATTEMPTS = 3
FETCH_RETRY_DELAY = 10
DAILY_TASK_MAX_RETRIES = 3
DAILY_TASK_RETRY_DELAY = 30
ATTENDANCE_POLL_INTERVAL = 600  # 10 minutes between attendance checks
WARNING_THRESHOLD_MINUTES = 30
DEFAULT_CLASS_DURATION_HOURS = 3


def get_hk_time():
    """Get current time in Hong Kong timezone"""
    return datetime.now(HONG_KONG_TZ)
```

- [ ] **Step 3: Verify**

```bash
.venv/bin/python -m py_compile config.py
.venv/bin/python -c "import config; assert config.SCHEDULE_TIME == '03:00'; assert config.get_hk_time().tzinfo is not None; print('config OK:', config.get_hk_time())"
```

Expected: prints `config OK: <current HKT datetime>` with `+08:00` offset.

- [ ] **Step 4: Commit**

```bash
git add config.py
git commit -m "refactor: extract config module (env, timezone, constants)"
```

---

### Task 2: `notify.py`

**Files:**
- Create: `notify.py`

**Interfaces:**
- Consumes: `config.DISCORD_WEBHOOK`
- Produces: `send_discord_notification(message: str) -> bool` — the ONLY function in the codebase that posts to the webhook. Returns `True` when sent (HTTP 204) or when no webhook is configured (message is logged instead); `False` on send failure.

- [ ] **Step 1: Write `notify.py`**

```python
import logging

import requests

from config import DISCORD_WEBHOOK

logger = logging.getLogger(__name__)


def send_discord_notification(message):
    """Send notification to Discord if webhook is configured"""
    if not DISCORD_WEBHOOK:
        logger.info("Discord notification (not sent): %s", message)
        return True

    try:
        # timeout added during review: a hung Discord endpoint must fail the
        # notification (return False), not block the calling thread forever
        response = requests.post(DISCORD_WEBHOOK, json={"content": message}, timeout=10)
        if response.status_code == 204:
            logger.info("Discord notification sent successfully")
            return True
        logger.error(
            "Failed to send Discord notification: %s - %s",
            response.status_code,
            response.text,
        )
        return False
    except Exception as e:
        logger.error("Error sending Discord notification: %s", e)
        return False
```

- [ ] **Step 2: Verify (no webhook configured locally → logs and returns True)**

```bash
.venv/bin/python -m py_compile notify.py
.venv/bin/python -c "
import logging; logging.basicConfig(level=logging.INFO)
from notify import send_discord_notification
assert send_discord_notification('smoke test') is True
print('notify OK')"
```

Expected: an INFO log line `Discord notification (not sent): smoke test`, then `notify OK`.

- [ ] **Step 3: Commit**

```bash
git add notify.py
git commit -m "refactor: extract single Discord notifier (dedupes webhook logic)"
```

---

### Task 3: `classes.py` (pure logic)

**Files:**
- Create: `classes.py`

**Interfaces:**
- Consumes: `config.HONG_KONG_TZ`, `config.get_hk_time`
- Produces (used by Tasks 5–6):
  - `parse_class_time(datetime_str: str) -> datetime | None` — parses `"%Y-%m-%d %H:%M"`, localized to HKT; `None` + error log on bad input.
  - `filter_today_classes(json_data: dict | None) -> dict | None` — same semantics as old code: passthrough on falsy input or `result != 1`; otherwise returns `{"result": ..., "classes": [...]}` keeping only today's sessions, dropping courses with no sessions left.
  - `build_class_infos(classes: dict | None) -> list[dict]` — list of dicts with keys `termcode`, `course_code`, `class_name`, `datetime` (tz-aware), `endtime` (tz-aware or None), `group`, `venue`, `url`. Empty list when input is falsy or `result != 1`.
  - `format_classes_message(classes: dict | None) -> str` — returns the exact old Discord text for the three cases: class list / `"No classes scheduled for today!"` / `"Failed to retrieve class information"`.

**No I/O, no Selenium, no requests in this module.**

- [ ] **Step 1: Write `classes.py`**

```python
import logging
from datetime import datetime

from config import HONG_KONG_TZ, get_hk_time

logger = logging.getLogger(__name__)


def parse_class_time(datetime_str):
    """Parse class datetime string and localize to Hong Kong timezone"""
    try:
        naive_dt = datetime.strptime(datetime_str, "%Y-%m-%d %H:%M")
        return HONG_KONG_TZ.localize(naive_dt)
    except ValueError as e:
        logger.error("Error parsing datetime: %s, error: %s", datetime_str, e)
        return None


def filter_today_classes(json_data):
    """Filter classes to only include those scheduled for today"""
    if not json_data:
        logger.info("No JSON data to filter")
        return json_data

    if json_data.get("result") != 1:
        logger.info(
            "API result is not successful (result: %s), returning unfiltered data",
            json_data.get("result"),
        )
        return json_data

    today_date = get_hk_time().date()
    logger.info("Filtering classes for today's date: %s", today_date)

    filtered_classes = []
    original_count = 0
    filtered_count = 0

    classes_list = json_data.get("classes", [])
    if not classes_list:
        logger.info("No classes found in the response")
        return json_data

    for course in classes_list:
        filtered_course = {
            "termcode": course.get("termcode", ""),
            "course_code": course.get("course_code", ""),
            "classes": [],
        }

        for class_session in course.get("classes", []):
            original_count += 1
            datetime_str = class_session.get("datetime", "")

            if not datetime_str:
                logger.warning(
                    "No datetime found for class: %s - %s",
                    course.get("course_code"),
                    class_session.get("name"),
                )
                continue

            class_datetime = parse_class_time(datetime_str)
            if not class_datetime:
                logger.warning(
                    "Could not parse datetime for class: %s - %s",
                    course.get("course_code"),
                    datetime_str,
                )
                continue

            if class_datetime.date() == today_date:
                filtered_course["classes"].append(class_session)
                filtered_count += 1
                logger.info(
                    "Keeping class: %s - %s at %s",
                    course.get("course_code"),
                    class_session.get("name"),
                    datetime_str,
                )
            else:
                logger.info(
                    "Filtering out class: %s - %s at %s (not today)",
                    course.get("course_code"),
                    class_session.get("name"),
                    datetime_str,
                )

        if filtered_course["classes"]:
            filtered_classes.append(filtered_course)

    logger.info(
        "Filtered classes: %d/%d classes are for today", filtered_count, original_count
    )

    return {
        "result": json_data.get("result", 0),
        "classes": filtered_classes,
    }


def build_class_infos(classes):
    """Turn filtered class data into a list of scheduling dicts"""
    class_infos = []

    if not (classes and classes.get("result") == 1):
        return class_infos

    for course in classes.get("classes", []):
        termcode = course.get("termcode", "")
        course_code = course.get("course_code", "")

        for class_session in course.get("classes", []):
            datetime_str = class_session.get("datetime", "")
            endtime_str = class_session.get("endtime", "")

            if not (datetime_str and termcode and course_code):
                continue

            class_datetime = parse_class_time(datetime_str)
            if not class_datetime:
                continue

            class_endtime = None
            if endtime_str:
                class_endtime = parse_class_time(endtime_str)
                if not class_endtime:
                    logger.error(
                        "Error parsing endtime for %s: %s", course_code, endtime_str
                    )

            class_infos.append(
                {
                    "termcode": termcode,
                    "course_code": course_code,
                    "class_name": class_session.get("name", ""),
                    "datetime": class_datetime,
                    "endtime": class_endtime,
                    "group": class_session.get("group", ""),
                    "venue": class_session.get("venue", ""),
                    "url": (
                        f"https://iole.hkmu.edu.hk/course{termcode}/{course_code}"
                        ".nsf//class_activities_student?readform&"
                    ),
                }
            )

    return class_infos


def format_classes_message(classes):
    """Build the Discord message describing today's classes"""
    if not (classes and classes.get("result") == 1):
        return "Failed to retrieve class information"

    class_list = classes.get("classes", [])
    if not class_list:
        return "No classes scheduled for today!"

    message = "**Retrieved Classes:**\n"
    message += f"`System Time: {get_hk_time().strftime('%Y-%m-%d %H:%M:%S %Z')}`\n\n"

    for course in class_list:
        course_code = course.get("course_code", "Unknown Course")
        for class_session in course.get("classes", []):
            class_name = class_session.get("name", "Unknown Class")
            datetime_str = class_session.get("datetime", "Unknown Time")
            endtime = class_session.get("endtime", "")
            venue = class_session.get("venue", "Unknown Venue")
            group = class_session.get("group", "")

            if datetime_str and datetime_str != "Unknown Time":
                try:
                    start_time = datetime_str.split(" ")[1]
                    end_time = endtime.split(" ")[1] if endtime else ""
                    time_range = (
                        f"{start_time} - {end_time}" if end_time else start_time
                    )

                    message += f"> **{course_code}** - {class_name}\n"
                    message += f">  TIME: {time_range}\n"
                    message += f">  VENUE: {venue}"
                    if group:
                        message += f" ({group})"
                    message += "\n"
                except Exception as e:
                    logger.error(
                        "Error processing class %s for Discord: %s", course_code, e
                    )
                    continue

    return message
```

- [ ] **Step 2: Verify with a behavior probe (uses today's real HKT date)**

```bash
.venv/bin/python -c "
import classes
from config import get_hk_time
today = get_hk_time().strftime('%Y-%m-%d')
data = {'result': 1, 'is_cc': False, 'classes': [
  {'termcode': '2504', 'course_code': 'COMP3120SEF', 'classes': [
    {'name': 'Lecture ( Full Time )', 'datetime': f'{today} 09:00', 'endtime': f'{today} 10:50', 'group': 'L01', 'venue': 'JCC D0212', 'host': 'x'},
    {'name': 'Old Lecture', 'datetime': '2020-01-01 09:00', 'endtime': '2020-01-01 10:50', 'group': 'L01', 'venue': 'JCC', 'host': 'x'}]}]}
f = classes.filter_today_classes(data)
assert len(f['classes']) == 1 and len(f['classes'][0]['classes']) == 1, f
infos = classes.build_class_infos(f)
assert len(infos) == 1, infos
assert infos[0]['url'] == 'https://iole.hkmu.edu.hk/course2504/COMP3120SEF.nsf//class_activities_student?readform&', infos[0]['url']
assert infos[0]['datetime'].tzinfo is not None and infos[0]['endtime'].tzinfo is not None
msg = classes.format_classes_message(f)
assert '**Retrieved Classes:**' in msg and '> **COMP3120SEF** - Lecture ( Full Time )' in msg, msg
assert '>  TIME: 09:00 - 10:50' in msg and '>  VENUE: JCC D0212 (L01)' in msg, msg
assert classes.format_classes_message(None) == 'Failed to retrieve class information'
assert classes.format_classes_message({'result': 0}) == 'Failed to retrieve class information'
assert classes.format_classes_message({'result': 1, 'classes': []}) == 'No classes scheduled for today!'
assert classes.parse_class_time('garbage') is None
assert classes.build_class_infos(None) == []
print('classes.py smoke OK')"
```

Expected: `classes.py smoke OK`.

- [ ] **Step 3: Commit**

```bash
git add classes.py
git commit -m "refactor: extract pure class parsing/filtering/formatting logic"
```

---

### Task 4: `ole.py` (driver, login, in-page fetch)

**Files:**
- Create: `ole.py`

**Interfaces:**
- Consumes: `config.OLE_URL`, `config.TODAY_CLASS_API_URL`, `config.LOGIN_PAGE_TIMEOUT`, `config.LOGIN_REDIRECT_TIMEOUT`, `config.FETCH_MAX_ATTEMPTS`, `config.FETCH_RETRY_DELAY`
- Produces (used by Tasks 5–6):
  - `create_driver() -> webdriver.Firefox` — headless Firefox, plain Selenium (no selenium-wire). Raises on failure (no silent fallback).
  - `login(driver, username, password) -> None` — navigates, waits for `userid`, submits credentials, waits for redirect. Raises `TimeoutException` (login page) / re-raises other errors. Does NOT quit the driver.
  - `enter_ole(username, password) -> webdriver.Firefox` — `create_driver()` + `login()`; quits the driver and re-raises if login fails. Same name/contract as the old function so callers read the same.
  - `fetch_today_classes(driver) -> dict | None` — up to `FETCH_MAX_ATTEMPTS` rounds of (in-page fetch, then navigation fallback), `FETCH_RETRY_DELAY` seconds apart; parsed JSON dict on success, `None` after all attempts fail.

- [ ] **Step 1: Write `ole.py`**

```python
import json
import logging
import time

from selenium import webdriver
from selenium.common.exceptions import TimeoutException
from selenium.webdriver.firefox.options import Options
from selenium.webdriver.support.ui import WebDriverWait

from config import (
    FETCH_MAX_ATTEMPTS,
    FETCH_RETRY_DELAY,
    LOGIN_PAGE_TIMEOUT,
    LOGIN_REDIRECT_TIMEOUT,
    OLE_URL,
    TODAY_CLASS_API_URL,
)

logger = logging.getLogger(__name__)

# Runs inside the logged-in page, so the browser attaches the same
# cookies/session the page's own getTodayClass XHR uses.
FETCH_SCRIPT = """
const url = arguments[0];
const done = arguments[arguments.length - 1];
fetch(url, {credentials: 'include'})
    .then(r => r.text())
    .then(text => done({ok: true, body: text}))
    .catch(err => done({ok: false, error: String(err)}));
"""


def create_driver():
    """Create a new headless Firefox WebDriver instance"""
    firefox_options = Options()
    firefox_options.add_argument("--disable-web-security")
    firefox_options.add_argument("--allow-running-insecure-content")

    # Docker/headless mode configurations
    firefox_options.add_argument("--headless")
    firefox_options.add_argument("--no-sandbox")
    firefox_options.add_argument("--disable-dev-shm-usage")
    firefox_options.add_argument("--disable-gpu")
    firefox_options.add_argument("--window-size=1920,1080")

    firefox_options.set_preference("network.proxy.type", 0)
    # Firefox's JSON viewer would replace the raw body when navigating
    # straight to the API URL, breaking the fallback fetch path
    firefox_options.set_preference("devtools.jsonview.enabled", False)
    firefox_options.set_preference("dom.webdriver.enabled", False)
    firefox_options.set_preference("useAutomationExtension", False)
    firefox_options.set_preference("dom.disable_beforeunload", True)
    firefox_options.set_preference("browser.tabs.remote.autostart", False)
    firefox_options.set_preference("browser.tabs.remote.autostart.2", False)

    driver = webdriver.Firefox(options=firefox_options)
    logger.info("WebDriver initialized successfully")
    return driver


def login(driver, username, password):
    """Log the given driver session into OLE. Raises on failure."""
    logger.info("Navigating to: %s", OLE_URL)
    driver.get(OLE_URL)
    logger.info("Navigation completed. Current URL: %s", driver.current_url)
    logger.info("Page title: %s", driver.title)

    if "404" in driver.title.lower() or "error" in driver.title.lower():
        logger.warning("Page title suggests an error: %s", driver.title)

    logger.info("Waiting for login page to load...")
    try:
        WebDriverWait(driver, LOGIN_PAGE_TIMEOUT).until(
            lambda d: d.find_element("id", "userid")
        )
    except TimeoutException:
        logger.error("Login page did not load properly - userid element not found")
        logger.error(
            "Current URL: %s, title: %s", driver.current_url, driver.title or "No title"
        )
        try:
            with open("/tmp/login_page_debug.html", "w", encoding="utf-8") as f:
                f.write(driver.page_source)
            logger.info("Page source saved to /tmp/login_page_debug.html for debugging")
        except Exception:
            logger.error("Could not save page source")
        raise
    logger.info("Login page loaded successfully")

    driver.find_element("id", "userid").send_keys(username)
    driver.find_element("id", "pwd").send_keys(password)
    driver.find_element("name", "loginButton2").click()
    logger.info("Login credentials submitted")

    logger.info("Waiting for login redirect...")
    try:
        WebDriverWait(driver, LOGIN_REDIRECT_TIMEOUT).until(
            lambda d: d.current_url != OLE_URL
        )
        logger.info("Redirected to: %s", driver.current_url)
    except Exception as e:
        logger.warning("Redirect timeout or error: %s", e)


def enter_ole(username, password):
    """Create a new driver session and login to OLE"""
    driver = create_driver()
    try:
        login(driver, username, password)
    except Exception:
        driver.quit()
        raise
    return driver


def _fetch_in_page(driver):
    """Primary path: run fetch() inside the logged-in page context"""
    try:
        driver.set_script_timeout(30)
        result = driver.execute_async_script(FETCH_SCRIPT, TODAY_CLASS_API_URL)
        if result and result.get("ok"):
            return json.loads(result["body"])
        logger.warning(
            "In-page fetch failed: %s",
            result.get("error") if result else "no result returned",
        )
    except Exception as e:
        logger.warning("In-page fetch raised: %s", e)
    return None


def _fetch_by_navigation(driver):
    """Fallback path: navigate straight to the API URL and read the body"""
    try:
        driver.get(TODAY_CLASS_API_URL)
        body_text = driver.execute_script("return document.body.textContent;")
        return json.loads(body_text)
    except Exception as e:
        logger.warning("Navigation fallback fetch failed: %s", e)
    return None


def fetch_today_classes(driver):
    """Fetch today's class JSON. Returns a dict, or None if all attempts fail."""
    for attempt in range(1, FETCH_MAX_ATTEMPTS + 1):
        logger.info(
            "Fetching today's classes (attempt %d/%d)", attempt, FETCH_MAX_ATTEMPTS
        )
        data = _fetch_in_page(driver)
        if data is None:
            data = _fetch_by_navigation(driver)
        if data is not None:
            logger.info("Fetched class data: %s", json.dumps(data, indent=2))
            return data
        if attempt < FETCH_MAX_ATTEMPTS:
            time.sleep(FETCH_RETRY_DELAY)

    logger.error(
        "Failed to fetch today's classes after %d attempts", FETCH_MAX_ATTEMPTS
    )
    return None
```

- [ ] **Step 2: Verify**

```bash
.venv/bin/python -m py_compile ole.py
.venv/bin/python -c "import ole; assert callable(ole.enter_ole) and callable(ole.fetch_today_classes); print('ole imports OK')"
```

Expected: `ole imports OK`. (Real login/fetch can only be verified against the live site — that's the manual live-run in Task 7.)

- [ ] **Step 3: Commit**

```bash
git add ole.py
git commit -m "refactor: extract Selenium module; replace selenium-wire sniffing with in-page fetch"
```

---

### Task 5: `attendance.py`

**Files:**
- Create: `attendance.py`

**Interfaces:**
- Consumes: `ole.enter_ole`, `notify.send_discord_notification`, `config.get_hk_time` + timing constants
- Produces (used by Task 6):
  - `schedule_attendance(class_infos: list[dict], username: str, password: str) -> None` — takes the `build_class_infos` output list (NOT the raw classes dict — this differs from the old signature).
  - `mark_attendance(class_info: dict, username: str, password: str) -> None` — same polling behavior as old code.

- [ ] **Step 1: Write `attendance.py`**

```python
import logging
import threading
import time
from datetime import timedelta

from selenium.common.exceptions import NoSuchElementException

from config import (
    ATTENDANCE_POLL_INTERVAL,
    DEFAULT_CLASS_DURATION_HOURS,
    ELEMENT_CHECK_WAIT,
    PAGE_LOAD_WAIT,
    POST_REFRESH_WAIT,
    WARNING_THRESHOLD_MINUTES,
    get_hk_time,
)
from notify import send_discord_notification
from ole import enter_ole

logger = logging.getLogger(__name__)


def schedule_attendance(class_infos, username, password):
    """Schedule attendance marking for each class"""
    logger.info("Total scheduled classes: %d", len(class_infos))
    logger.info(
        "Current System Time (HKT): %s",
        get_hk_time().strftime("%Y-%m-%d %H:%M:%S %Z"),
    )
    for class_info in class_infos:
        _schedule_class(class_info, username, password)


def _schedule_class(class_info, username, password):
    """Schedule attendance marking for a specific class"""
    class_time = class_info["datetime"]
    class_endtime = class_info.get("endtime")
    current_time = get_hk_time()

    logger.info("System Time: %s", current_time.strftime("%Y-%m-%d %H:%M:%S %Z"))
    logger.info("Class Time: %s", class_time.strftime("%Y-%m-%d %H:%M:%S %Z"))

    if not class_endtime:
        class_endtime = class_time + timedelta(hours=DEFAULT_CLASS_DURATION_HOURS)
        logger.info(
            "Class End Time (estimated): %s",
            class_endtime.strftime("%Y-%m-%d %H:%M:%S %Z"),
        )
    else:
        logger.info(
            "Class End Time: %s", class_endtime.strftime("%Y-%m-%d %H:%M:%S %Z")
        )

    delay = (class_time - current_time).total_seconds()

    if delay > 0:
        logger.info(
            "Scheduling attendance for %s in %.1f minutes",
            class_info["course_code"],
            delay / 60,
        )
        timer = threading.Timer(
            delay, mark_attendance, args=[class_info, username, password]
        )
        timer.daemon = True
        timer.start()
    elif current_time <= class_endtime:
        minutes_since_start = (current_time - class_time).total_seconds() / 60
        minutes_until_end = (class_endtime - current_time).total_seconds() / 60
        logger.info(
            "Class %s is IN PROGRESS! Started %.0f minutes ago, ends in %.0f minutes. "
            "Attempting attendance immediately...",
            class_info["course_code"],
            minutes_since_start,
            minutes_until_end,
        )
        attendance_thread = threading.Thread(
            target=mark_attendance, args=[class_info, username, password]
        )
        attendance_thread.daemon = True
        attendance_thread.start()
    else:
        minutes_since_end = (current_time - class_endtime).total_seconds() / 60
        logger.info(
            "Class %s has already ENDED %.0f minutes ago",
            class_info["course_code"],
            minutes_since_end,
        )


def _maybe_send_warning(class_info, class_endtime, attempt, warning_sent, error_occurred):
    """Send the one-time 30-minute warning if due. Returns updated warning_sent."""
    if warning_sent:
        return True

    minutes_remaining = (class_endtime - get_hk_time()).total_seconds() / 60
    if minutes_remaining > WARNING_THRESHOLD_MINUTES:
        return False

    logger.warning("Only %.0f minutes left in class!", minutes_remaining)

    warning_message = "**- ATTENDANCE WARNING**\n"
    warning_message += f"> Course: {class_info['course_code']}\n"
    warning_message += f"> Class: {class_info['class_name']}\n"
    warning_message += f"> Only {minutes_remaining:.0f} minutes remaining!\n"
    if error_occurred:
        warning_message += f"ERROR occurred during attempt {attempt + 1}\n"
        warning_message += f"Total attempts: {attempt + 1}"
    else:
        warning_message += f"> Attempts so far: {attempt + 1}"

    send_discord_notification(warning_message)
    return True


def mark_attendance(class_info, username, password):
    """Mark attendance for a specific class"""
    logger.info(
        "Starting attendance for %s - %s",
        class_info["course_code"],
        class_info["class_name"],
    )
    logger.info(
        "Attendance Start Time: %s", get_hk_time().strftime("%Y-%m-%d %H:%M:%S %Z")
    )

    driver = None
    try:
        driver = enter_ole(username, password)

        class_url = class_info["url"]
        logger.info("Navigating to: %s", class_url)
        driver.get(class_url)

        logger.info("Waiting for page to load...")
        time.sleep(PAGE_LOAD_WAIT)

        class_endtime = class_info.get("endtime")
        if not class_endtime:
            class_endtime = class_info["datetime"] + timedelta(
                hours=DEFAULT_CLASS_DURATION_HOURS
            )

        logger.info("Will check for attendance submission until: %s", class_endtime)
        attempt = 0
        warning_sent = False
        # guards the FAILED message: a success in the final seconds of class
        # must never be followed by a spurious FAILED notification
        confirmed = False

        while get_hk_time() < class_endtime:
            try:
                logger.info(
                    "Attempt %d: Checking if attendance has been submitted...",
                    attempt + 1,
                )
                time.sleep(ELEMENT_CHECK_WAIT)

                try:
                    driver.find_element("id", "submitted_msg")
                    logger.info("Found attendance confirmation element (submitted_msg)!")

                    success_message = "**- Attendance Confirmed!**\n"
                    success_message += f"> Course: {class_info['course_code']}\n"
                    success_message += f"> Class: {class_info['class_name']}\n"
                    success_message += (
                        f"> Time: {get_hk_time().strftime('%Y-%m-%d %H:%M:%S %Z')}\n"
                    )
                    success_message += "> Status: Attendance successfully submitted"
                    send_discord_notification(success_message)

                    logger.info(
                        "Attendance confirmed for %s", class_info["course_code"]
                    )
                    confirmed = True
                    break
                except NoSuchElementException:
                    logger.info(
                        "Attendance not yet submitted (submitted_msg element not found)"
                    )

                warning_sent = _maybe_send_warning(
                    class_info, class_endtime, attempt, warning_sent,
                    error_occurred=False,
                )

                minutes_remaining = (
                    class_endtime - get_hk_time()
                ).total_seconds() / 60
                logger.info(
                    "Attendance not submitted yet, waiting 10 minutes... "
                    "(%.0f minutes left)",
                    minutes_remaining,
                )
                time.sleep(ATTENDANCE_POLL_INTERVAL)
                driver.refresh()
                logger.info("Page refreshed, waiting for reload...")
                time.sleep(POST_REFRESH_WAIT)
                attempt += 1

            except Exception as e:
                logger.error(
                    "Error during attendance attempt %d: %s", attempt + 1, e
                )
                warning_sent = _maybe_send_warning(
                    class_info, class_endtime, attempt, warning_sent,
                    error_occurred=True,
                )
                attempt += 1
                time.sleep(ATTENDANCE_POLL_INTERVAL)

        if not confirmed and get_hk_time() >= class_endtime:
            fail_message = "**- Attendance FAILED**\n"
            fail_message += f"> Course: {class_info['course_code']}\n"
            fail_message += (
                "> Status: Attendance submission not confirmed by end of class\n"
            )
            fail_message += f"> Total attempts: {attempt}"
            send_discord_notification(fail_message)

    except Exception as e:
        logger.error(
            "Error marking attendance for %s: %s", class_info["course_code"], e
        )
        error_message = "**- Attendance ERROR**\n"
        error_message += f"> Course: {class_info['course_code']}\n"
        error_message += f"`Error: {e}`"
        send_discord_notification(error_message)

    finally:
        if driver:
            try:
                driver.quit()
                logger.info("Driver cleaned up for %s", class_info["course_code"])
            except Exception as e:
                logger.error("Error cleaning up driver: %s", e)
```

Note the success-`break` interaction: after a successful confirmation the loop breaks while `get_hk_time() < class_endtime` is still true, so the FAILED message is not sent — same as the old code.

- [ ] **Step 2: Verify**

```bash
.venv/bin/python -m py_compile attendance.py
.venv/bin/python -c "
import logging; logging.basicConfig(level=logging.INFO)
import attendance
from datetime import timedelta
from config import get_hk_time
# already-ended class must be skipped without touching Selenium
past = {'course_code': 'TEST101', 'class_name': 'T', 'datetime': get_hk_time() - timedelta(hours=5), 'endtime': get_hk_time() - timedelta(hours=4), 'group': '', 'venue': '', 'termcode': 'x', 'url': 'x'}
attendance.schedule_attendance([past], 'u', 'p')
print('attendance smoke OK')"
```

Expected: log line `Class TEST101 has already ENDED ...`, then `attendance smoke OK`. No browser launches.

- [ ] **Step 3: Commit**

```bash
git add attendance.py
git commit -m "refactor: extract attendance scheduling and marking; dedupe warning logic"
```

---

### Task 6: Rewrite `main.py` (orchestration, retry fix, logging setup, SCHEDULE_TIME)

**Files:**
- Modify: `main.py` (full replacement of all 635 lines)

**Interfaces:**
- Consumes: everything produced by Tasks 1–5:
  - `config` settings/constants, `config.get_hk_time`
  - `notify.send_discord_notification(message) -> bool`
  - `classes.filter_today_classes(dict|None) -> dict|None`, `classes.build_class_infos(dict|None) -> list[dict]`, `classes.format_classes_message(dict|None) -> str`
  - `ole.enter_ole(username, password) -> driver`, `ole.fetch_today_classes(driver) -> dict|None`
  - `attendance.schedule_attendance(list[dict], username, password) -> None`
- Produces: the executable entrypoint (`python main.py`), `daily_attendance_task()` for the scheduler.

- [ ] **Step 1: Replace `main.py` entirely with:**

```python
import logging
import time

import schedule

import config
from attendance import schedule_attendance
from classes import build_class_infos, filter_today_classes, format_classes_message
from notify import send_discord_notification
from ole import enter_ole, fetch_today_classes

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(name)s: %(message)s",
)
logger = logging.getLogger(__name__)


def _login_with_retries():
    """Login with a fresh driver, retrying on failure. Returns the driver."""
    last_error = None
    for attempt in range(1, config.LOGIN_MAX_RETRIES + 1):
        try:
            logger.info("Login attempt %d/%d", attempt, config.LOGIN_MAX_RETRIES)
            driver = enter_ole(config.STUDENT_ID, config.STUDENT_PASSWORD)
            logger.info("Login successful!")
            return driver
        except Exception as e:
            last_error = e
            logger.error("Login attempt %d failed: %s", attempt, e)
            if attempt < config.LOGIN_MAX_RETRIES:
                logger.info("Retrying in %d seconds...", config.LOGIN_RETRY_DELAY)
                time.sleep(config.LOGIN_RETRY_DELAY)
    raise last_error


def _fetch_classes_once():
    """One login+fetch cycle. Returns today's filtered classes dict, or None."""
    driver = _login_with_retries()
    try:
        raw = fetch_today_classes(driver)
    finally:
        driver.quit()
        logger.info("Fetch driver session closed")

    if raw is None:
        return None
    return filter_today_classes(raw)


def daily_attendance_task():
    """Runs daily at SCHEDULE_TIME to set up attendance for the day"""
    if not config.STUDENT_ID or not config.STUDENT_PASSWORD:
        error_msg = (
            "MISSING required environment variables: STUDENT_ID and/or STUDENT_PASSWORD"
        )
        logger.error(error_msg)
        send_discord_notification(f"**Configuration Error**\n{error_msg}")
        return

    if not config.DISCORD_WEBHOOK:
        logger.warning("DISCORD_WEBHOOK not set, notifications disabled")

    try:
        classes_data = None
        for attempt in range(1, config.DAILY_TASK_MAX_RETRIES + 1):
            logger.info(
                "Class retrieval cycle %d/%d", attempt, config.DAILY_TASK_MAX_RETRIES
            )
            classes_data = _fetch_classes_once()
            if classes_data and classes_data.get("result") == 1:
                break
            logger.warning("Class retrieval cycle %d did not succeed", attempt)
            if attempt < config.DAILY_TASK_MAX_RETRIES:
                logger.info(
                    "Retrying in %d seconds...", config.DAILY_TASK_RETRY_DELAY
                )
                time.sleep(config.DAILY_TASK_RETRY_DELAY)

        logger.info("Sending class notification to Discord...")
        send_discord_notification(format_classes_message(classes_data))

        if classes_data and classes_data.get("result") == 1:
            logger.info("Scheduling attendance for today's classes...")
            class_infos = build_class_infos(classes_data)
            schedule_attendance(
                class_infos, config.STUDENT_ID, config.STUDENT_PASSWORD
            )

    except Exception as e:
        logger.exception("ERROR during daily setup")
        error_message = "**Daily Setup Error**\n"
        error_message += (
            f"Date: {config.get_hk_time().strftime('%Y-%m-%d %H:%M:%S %Z')}\n"
        )
        error_message += f"Error: {e}"
        send_discord_notification(error_message)


def main():
    logger.info(
        "System Time (HKT): %s",
        config.get_hk_time().strftime("%Y-%m-%d %H:%M:%S %Z"),
    )
    logger.info("Initial attendance setup...")
    daily_attendance_task()

    schedule.every().day.at(config.SCHEDULE_TIME).do(daily_attendance_task)
    logger.info("Scheduled daily attendance setup at %s", config.SCHEDULE_TIME)
    logger.info("Bot will keep running to handle scheduled tasks... Press Ctrl+C to stop")

    try:
        while True:
            schedule.run_pending()
            time.sleep(60)
    except KeyboardInterrupt:
        logger.info("Stopped by user")


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Verify the whole module graph**

```bash
.venv/bin/python -m py_compile config.py notify.py classes.py ole.py attendance.py main.py
.venv/bin/python -c "import main; assert callable(main.daily_attendance_task); print('main imports OK')"
.venv/bin/python -c "
# With no credentials set locally, daily_attendance_task must exit early
# via the Configuration Error path without launching a browser.
import main
main.daily_attendance_task()
print('early-exit path OK')"
```

Expected: `main imports OK`; then the error log `MISSING required environment variables...`, the logged (not sent) Configuration Error notification, and `early-exit path OK`. If a local `.env` exists with real credentials, skip the third command (it would attempt a real login).

- [ ] **Step 3: Confirm no selenium-wire references remain**

```bash
grep -rn "seleniumwire\|selenium_wire\|selenium-wire" --include="*.py" --exclude-dir=.venv . ; echo "exit: $?"
```

Expected: no matches, `exit: 1`.

- [ ] **Step 4: Commit**

```bash
git add main.py
git commit -m "refactor: main.py becomes thin orchestrator; fix retry recursion; honor SCHEDULE_TIME"
```

---

### Task 7: Dependencies, Dockerfile, build verification

**Files:**
- Modify: `requirements.txt` (full replacement)
- Modify: `Dockerfile` (one line)

**Interfaces:**
- Consumes: the six modules from Tasks 1–6.
- Produces: a buildable Docker image identical in behavior to the old deployment.

- [ ] **Step 1: Replace `requirements.txt` entirely with:**

```
selenium>=4.27
requests>=2.32.5
schedule>=1.2.2
python-dotenv>=1.1.1
pytz>=2025.2
```

- [ ] **Step 2: In `Dockerfile`, change the app copy line**

Old (line 27):
```dockerfile
COPY main.py .
```

New:
```dockerfile
COPY *.py .
```

No other Dockerfile changes.

- [ ] **Step 3: Keep the local venv out of the Docker build context, then build**

`.dockerignore` is gitignored in this repo (stays local-only, do not commit it):

```bash
printf '.venv\n.git\ndocs\n' > .dockerignore
docker build -t hkmu-ole-attendance:refactor .
```

Expected: build completes successfully (downloads geckodriver + installs the five deps; note there are no longer `blinker`/`pyOpenSSL` pins in the pip output).

- [ ] **Step 4: Verify imports inside the image**

```bash
docker run --rm --entrypoint python hkmu-ole-attendance:refactor -c "import main; print('image imports OK')"
```

Expected: `image imports OK`.

- [ ] **Step 5: Commit**

```bash
git add requirements.txt Dockerfile
git commit -m "refactor: drop selenium-wire and its blinker/pyOpenSSL pins; copy all modules in Docker"
```

- [ ] **Step 6: Manual live verification (user action — cannot be automated)**

Run the container with a real `.env` (credentials + webhook). Confirm:
1. Login succeeds and the class list arrives (in-page fetch, or the navigation fallback — check logs for which path fired).
2. The Discord "Retrieved Classes" message matches the old format.
3. If neither fetch path works against the live site, the revert path is: restore selenium-wire in `ole.py` + `requirements.txt` only (all other modules are unaffected).
