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
