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
