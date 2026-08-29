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
