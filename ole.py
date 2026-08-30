import json
import logging
import time

from selenium import webdriver
from selenium.common.exceptions import TimeoutException
from selenium.webdriver.firefox.options import Options
from selenium.webdriver.support.ui import WebDriverWait

from config import (
    DASHBOARD_LOAD_TIMEOUT,
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
fetch(url, {
    credentials: 'include',
    // without the XHR marker the API answers {"error": "9901",
    // "errormsg": "The API is only for web users."}
    headers: {'X-Requested-With': 'XMLHttpRequest'}
})
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

    # The SSO redirect chain lands on a transient page before the dashboard;
    # the oleconnect API session is only established once the dashboard loads,
    # so fetching (or navigating) earlier gets rejected or races an unload.
    logger.info("Waiting for OLE dashboard to load...")
    try:
        WebDriverWait(driver, DASHBOARD_LOAD_TIMEOUT).until(
            lambda d: "myOLE.nsf" in d.current_url
            and d.execute_script("return document.readyState") == "complete"
        )
        logger.info("Dashboard loaded: %s", driver.current_url)
    except Exception as e:
        logger.warning("Dashboard load wait gave up: %s (continuing)", e)


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
    """Fetch today's class JSON. Retries until the API reports success
    (result == 1); returns the last payload (or None) after all attempts,
    so callers can still report the API's own error."""
    last_data = None
    for attempt in range(1, FETCH_MAX_ATTEMPTS + 1):
        logger.info(
            "Fetching today's classes (attempt %d/%d)", attempt, FETCH_MAX_ATTEMPTS
        )
        data = _fetch_in_page(driver)
        if data is None:
            # navigation moves the page off the dashboard, so only fall back
            # when the in-page transport itself failed, not on an API error
            data = _fetch_by_navigation(driver)
        if isinstance(data, dict):
            last_data = data
            if data.get("result") == 1:
                logger.info("Fetched class data: %s", json.dumps(data, indent=2))
                return data
            logger.warning("API returned unsuccessful payload: %s", json.dumps(data))
        if attempt < FETCH_MAX_ATTEMPTS:
            time.sleep(FETCH_RETRY_DELAY)

    logger.error(
        "No successful class payload after %d attempts", FETCH_MAX_ATTEMPTS
    )
    return last_data
