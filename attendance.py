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
