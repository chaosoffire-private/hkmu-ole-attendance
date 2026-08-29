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
