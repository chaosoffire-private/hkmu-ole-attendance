import json
import os
import time
import requests
import threading
import schedule
from datetime import datetime, timedelta
import pytz
from dotenv import load_dotenv
from seleniumwire import webdriver
from selenium.webdriver.firefox.options import Options
from selenium.webdriver.support.ui import WebDriverWait
from selenium.common.exceptions import NoSuchElementException, TimeoutException

# Load environment variables from .env file
load_dotenv()

# Configuration from environment variables
OLE_URL = os.getenv("OLE_URL", "https://iole.hkmu.edu.hk")
DISCORD_WEBHOOK = os.getenv("DISCORD_WEBHOOK", "")
STUDENT_ID = os.getenv("STUDENT_ID", "")
STUDENT_PASSWORD = os.getenv("STUDENT_PASSWORD", "")

# Timezone configuration
HONG_KONG_TZ = pytz.timezone('Asia/Hong_Kong')

def get_hk_time():
    """Get current time in Hong Kong timezone"""
    return datetime.now(HONG_KONG_TZ)

def parse_class_time(datetime_str):
    """Parse class datetime string and localize to Hong Kong timezone"""
    try:
        # Parse the datetime string (assumes it's in Hong Kong time)
        naive_dt = datetime.strptime(datetime_str, "%Y-%m-%d %H:%M")
        # Localize to Hong Kong timezone
        return HONG_KONG_TZ.localize(naive_dt)
    except ValueError as e:
        print(f"Error parsing datetime: {datetime_str}, error: {e}")
        return None

def send_discord_notification(message):
    """Send notification to Discord if webhook is configured"""
    if not DISCORD_WEBHOOK:
        print(f"Discord notification (not sent): {message}")
        return True
    
    try:
        payload = {"content": message}
        response = requests.post(DISCORD_WEBHOOK, json=payload)
        if response.status_code == 204:
            print("Discord notification sent successfully")
            return True
        else:
            print(f"Failed to send Discord notification: {response.status_code} - {response.text}")
            return False
    except Exception as e:
        print(f"Error sending Discord notification: {e}")
        return False

def create_driver():
    """Create a new Firefox WebDriver instance"""
    # Configure Firefox options
    firefox_options = Options()
    firefox_options.add_argument('--disable-web-security')
    firefox_options.add_argument('--allow-running-insecure-content')
    
    # Docker/headless mode configurations
    firefox_options.add_argument('--headless')  # Run in headless mode
    firefox_options.add_argument('--no-sandbox')
    firefox_options.add_argument('--disable-dev-shm-usage')
    firefox_options.add_argument('--disable-gpu')
    firefox_options.add_argument('--window-size=1920,1080')
    
    # Add proxy bypass for potential issues
    firefox_options.set_preference("network.proxy.type", 0)  # Direct connection, no proxy

    # Configure seleniumwire options - minimal configuration to avoid proxy issues
    seleniumwire_options = {
        'addr': '127.0.0.1',
        'port': 0,  # Let selenium-wire choose an available port
        'disable_encoding': True,
        'suppress_connection_errors': True,
        'verify_ssl': False,
        'connection_timeout': 50,
    }

    try:
        driver = webdriver.Firefox(
            options=firefox_options,
            seleniumwire_options=seleniumwire_options
        )
        print("WebDriver initialized successfully")
        return driver
    except Exception as e:
        print(f"Error initializing WebDriver: {e}")
        # Fallback: try without seleniumwire options
        print("Trying fallback initialization...")
        return webdriver.Firefox(options=firefox_options)

def enter_ole(username, password):
    """Create new driver session and login to OLE"""
    driver = create_driver()
    
    driver.get(OLE_URL)

    # Login
    driver.find_element("id", "userid").send_keys(username)
    driver.find_element("id", "pwd").send_keys(password)
    driver.find_element("name", "loginButton2").click()

    # Wait for page to redirect after login
    print("Waiting for login redirect...")
    try:
        WebDriverWait(driver, 10).until(
            lambda d: d.current_url != OLE_URL
        )
        print(f"Redirected to: {driver.current_url}")
        
        # sometimes xhr request doesn't load fast enough (lol)
        time.sleep(30)
        
    except Exception as e:
        print(f"Redirect timeout or error: {e}")
    
    return driver

def getTodayClasses(driver, url):
    print(f"\nSearching for XHR request to: {url}")
    
    target_request = None
    
    for request in driver.requests:
        if request.response and url in request.url:
            target_request = request
            break
    
    if target_request:
        print(f"-- Found getTodayClass request --")
        print(f"{target_request.method} {target_request.response.status_code}: {target_request.url}")
        
        # Show response body
        if target_request.response.body:
            try:
                body = target_request.response.body.decode('utf-8')
                json_data = json.loads(body)
                print(f"Response: {json.dumps(json_data, indent=2)}")
                return json_data
                
            except json.JSONDecodeError:
                print(f"Response Body (not JSON): {body}")
    else:
        print("ERROR!!!! No getTodayClass request found!")

def send_classes(classes):
    """Send Discord webhook notification about today's classes"""
    if classes and classes.get("result") == 1:
        class_list = classes.get("classes", [])
        if class_list:
            message = "**Retrieved Classes:**\n"
            message += f"`System Time: {get_hk_time().strftime('%Y-%m-%d %H:%M:%S %Z')}`\n\n"
            for course in class_list:
                course_code = course.get("course_code", "Unknown Course")
                for class_session in course.get("classes", []):
                    class_name = class_session.get("name", "Unknown Class")
                    datetime = class_session.get("datetime", "Unknown Time")
                    endtime = class_session.get("endtime", "")
                    venue = class_session.get("venue", "Unknown Venue")
                    group = class_session.get("group", "")
                    
                    # Format time nicely
                    if datetime:
                        try:
                            start_time = datetime.split(" ")[1]  # Get time part
                            end_time = endtime.split(" ")[1] if endtime else ""
                            time_range = f"{start_time} - {end_time}" if end_time else start_time
                        except:
                            time_range = datetime
                    else:
                        time_range = "Unknown Time"
                    
                    message += f"> **{course_code}** - {class_name}\n"
                    message += f">  TIME: {time_range}\n"
                    message += f">  VENUE: {venue}"
                    if group:
                        message += f" ({group})"
                    message += "\n\n"

        else:
            message = "No classes scheduled for today!"
    else:
        message = "Failed to retrieve class information"
    
    # Only send to Discord if webhook is configured
    if not DISCORD_WEBHOOK:
        print("Discord webhook not configured, skipping notification")
        print(f"Classes message: {message}")
        return True
        
    payload = {
        "content": message
    }

    response = requests.post(DISCORD_WEBHOOK, json=payload)
    if response.status_code == 204:
        print("Discord webhook sent successfully")
        return True
    else:
        print(f"Failed to send Discord webhook: {response.status_code} - {response.text}")
        return False
    
def schedule_attendance(classes, username, password):
    """Parse classes and schedule attendance marking"""
    scheduled_classes = []
    
    if classes and classes.get("result") == 1:
        class_list = classes.get("classes", [])
        
        for course in class_list:
            termcode = course.get("termcode", "")
            course_code = course.get("course_code", "")
            
            for class_session in course.get("classes", []):
                class_name = class_session.get("name", "")
                datetime_str = class_session.get("datetime", "")
                endtime_str = class_session.get("endtime", "")
                group = class_session.get("group", "")
                venue = class_session.get("venue", "")
                
                if datetime_str and termcode and course_code:
                    try:
                        class_datetime = parse_class_time(datetime_str)
                        if not class_datetime:
                            continue
                        
                        class_endtime = None
                        if endtime_str:
                            class_endtime = parse_class_time(endtime_str)
                            if not class_endtime:
                                print(f"Error parsing endtime for {course_code}: {endtime_str}")
                        
                        class_info = {
                            "termcode": termcode,
                            "course_code": course_code,
                            "class_name": class_name,
                            "datetime": class_datetime,
                            "endtime": class_endtime,
                            "group": group,
                            "venue": venue,
                            "url": f"https://iole.hkmu.edu.hk/{termcode}/{course_code}.nsf//class_activities_student?readform&"
                        }
                        
                        scheduled_classes.append(class_info)
                        print(f"Scheduled: {course_code} - {class_name} at {class_datetime}")
                        
                    except ValueError as e:
                        print(f"Error parsing datetime for {course_code}: {e}")
    
    print(f"\nTotal scheduled classes: {len(scheduled_classes)}")
    print(f"🕐 Current System Time (HKT): {get_hk_time().strftime('%Y-%m-%d %H:%M:%S %Z')}")
    
    # Schedule attendance for each class
    for class_info in scheduled_classes:
        schedule_class_attendance(class_info, username, password)

def schedule_class_attendance(class_info, username, password):
    """Schedule attendance marking for a specific class"""
    class_time = class_info["datetime"]
    current_time = get_hk_time()
    
    print(f"🕐 System Time: {current_time.strftime('%Y-%m-%d %H:%M:%S %Z')}")
    print(f"📅 Class Time: {class_time.strftime('%Y-%m-%d %H:%M:%S %Z') if hasattr(class_time, 'tzinfo') and class_time.tzinfo else class_time.strftime('%Y-%m-%d %H:%M:%S')}")
    
    # Calculate delay until class starts
    delay = (class_time - current_time).total_seconds()
    
    if delay > 0:
        print(f"Scheduling attendance for {class_info['course_code']} in {delay/60:.1f} minutes")
        
        # Schedule the attendance marking
        timer = threading.Timer(delay, mark_attendance, args=[class_info, username, password])
        timer.daemon = True
        timer.start()
    else:
        print(f"Class {class_info['course_code']} has already started or passed")

def mark_attendance(class_info, username, password):
    """Mark attendance for a specific class"""
    print(f"\n🎯 Starting attendance for {class_info['course_code']} - {class_info['class_name']}")
    print(f"🕐 Attendance Start Time: {get_hk_time().strftime('%Y-%m-%d %H:%M:%S %Z')}")
    
    # Create new driver session and login to OLE
    driver = None
    try:
        driver = enter_ole(username, password)
        
        class_url = class_info["url"]
        print(f"Navigating to: {class_url}")
        driver.get(class_url)
        
        # Try to find "present" every 10 minutes until class endtime
        current_time = get_hk_time()
        class_endtime = class_info.get("endtime")
        
        # If no endtime specified, default to 3 hours after class start
        if not class_endtime:
            class_endtime = class_info["datetime"] + timedelta(hours=3)
        
        print(f"Will check for attendance until: {class_endtime}")
        print(f"🕐 Current Time: {current_time.strftime('%Y-%m-%d %H:%M:%S %Z')}")
        attempt = 0
        warning_sent = False  # Track if we've sent the 30-minute warning
        
        while current_time < class_endtime:
            try:
                print(f"Attempt {attempt + 1}: Checking if 'present' exists on page...")
                
                # Check if the word "present" exists anywhere on the page
                page_source = driver.page_source.lower()
                
                if "present" in page_source:
                    print(f"Found word 'present' on the page!")
                    
                    # Send success notification
                    success_message = f"**Attendance Checked!**\n"
                    success_message += f"Course: {class_info['course_code']}\n"
                    success_message += f"Class: {class_info['class_name']}\n"
                    success_message += f"Time: {get_hk_time().strftime('%Y-%m-%d %H:%M:%S %Z')}\n"
                    success_message += f"Status: Word 'present' found on attendance page"
                    
                    send_discord_notification(success_message)
                    
                    print(f"'Present' found for {class_info['course_code']}")
                    break
                else:
                    # Check if we're in the last 30 minutes and haven't sent warning yet
                    time_remaining = (class_endtime - current_time).total_seconds()
                    minutes_remaining = time_remaining / 60
                    
                    if minutes_remaining <= 30 and not warning_sent:
                        print(f"⚠️ WARNING: Only {minutes_remaining:.0f} minutes left in class!")
                        
                        # Send warning notification
                        warning_message = f"⚠️ **ATTENDANCE WARNING**\n"
                        warning_message += f"Course: {class_info['course_code']}\n"
                        warning_message += f"Class: {class_info['class_name']}\n"
                        warning_message += f"Only {minutes_remaining:.0f} minutes remaining!\n"
                        warning_message += f"Still searching for 'present' on attendance page\n"
                        warning_message += f"Attempts so far: {attempt + 1}"
                        
                        send_discord_notification(warning_message)
                        warning_sent = True
                    
                    print(f"Word 'present' not found on page, waiting 10 minutes... ({minutes_remaining:.0f} minutes left)")
                    time.sleep(600)  # Wait 10 minutes
                    driver.refresh()  # Refresh the page
                    attempt += 1
                    current_time = get_hk_time()  # Update current time
                    
            except Exception as e:
                print(f"Error during attendance attempt {attempt + 1}: {e}")
                
                # Check for warning even during errors
                time_remaining = (class_endtime - current_time).total_seconds()
                minutes_remaining = time_remaining / 60
                
                if minutes_remaining <= 30 and not warning_sent:
                    print(f"⚠️ WARNING: Only {minutes_remaining:.0f} minutes left in class!")
                    
                    # Send warning notification
                    warning_message = f"**ATTENDANCE WARNING**\n"
                    warning_message += f"Course: {class_info['course_code']}\n"
                    warning_message += f"Class: {class_info['class_name']}\n"
                    warning_message += f"Only {minutes_remaining:.0f} minutes remaining!\n"
                    warning_message += f"ERROR occurred during attempt {attempt + 1}\n"
                    warning_message += f"Total attempts: {attempt + 1}"
                    
                    send_discord_notification(warning_message)
                    warning_sent = True
                
                attempt += 1
                time.sleep(600)  # Wait 10 minutes before retry
                current_time = get_hk_time()  # Update current time
        
        # Send failure notification if time exceeded without finding "present"
        if current_time >= class_endtime:
            fail_message = f"**Attendance FAILED**\n"
            fail_message += f"Course: {class_info['course_code']}\n"
            fail_message += f"Word 'present' not found on page until class end time ({class_endtime})\n"
            fail_message += f"Total attempts: {attempt}"
            
            send_discord_notification(fail_message)
            
    except Exception as e:
        print(f"Error marking attendance for {class_info['course_code']}: {e}")
        
        error_message = f"**Attendance ERROR**\n"
        error_message += f"Course: {class_info['course_code']}\n"
        error_message += f"Error: {str(e)}"
        
        send_discord_notification(error_message)
    
    finally:
        if driver:
            try:
                driver.quit()
                print(f"Driver cleaned up for {class_info['course_code']}")
            except Exception as e:
                print(f"Error cleaning up driver: {e}")



def daily_attendance_task():
    """Main function that runs daily at 3 AM to set up attendance for the day"""
    print(f"\n{'='*60}")
    print(f"- DAILY ATTENDANCE SETUP - {get_hk_time().strftime('%Y-%m-%d %H:%M:%S %Z')}")
    print(f"{'='*60}")
    
    if not STUDENT_ID or not STUDENT_PASSWORD:
        error_msg = "MISSING required environment variables: STUDENT_ID and/or STUDENT_PASSWORD"
        print(error_msg)
        send_discord_notification(f"**Configuration Error**\n{error_msg}")
        return
    
    if not DISCORD_WEBHOOK:
        print("⚠️ Warning: DISCORD_WEBHOOK not set, notifications disabled")

    try:
        # Create initial driver session for getting classes
        print("- Creating initial driver session...")
        initial_driver = enter_ole(STUDENT_ID, STUDENT_PASSWORD)

        # get json data for classes
        print("- Retrieving today's classes...")
        classes = getTodayClasses(initial_driver, "https://oleconnect.hkmu.edu.hk/oledb/api/getTodayClass/")
        
        # Close initial driver as we're done with it
        initial_driver.quit()
        print("- Initial driver session closed")
        
        # Send classes to Discord
        print("- Sending class notification to Discord...")
        send_classes(classes)

        # Schedule
        print("- Scheduling attendance for today's classes...")
        schedule_attendance(classes, STUDENT_ID, STUDENT_PASSWORD)
        
        print(f"- Daily setup completed successfully!")
        print(f"{'='*60}\n")
        
    except Exception as e:
        print(f"- ERROR during daily setup: {e}")
        
        error_message = f"🚨 **Daily Setup Error**\n"
        error_message += f"Date: {get_hk_time().strftime('%Y-%m-%d %H:%M:%S %Z')}\n"
        error_message += f"Error: {str(e)}"
        
        send_discord_notification(error_message)

if __name__ == "__main__":
    print(f"- System Time (HKT): {get_hk_time().strftime('%Y-%m-%d %H:%M:%S %Z')}")
    print("- Initial attendance setup...")
    daily_attendance_task()
    
    # Schedule the daily task to run at 3:00 AM every day
    schedule.every().day.at("03:00").do(daily_attendance_task)
    
    print("Scheduled daily attendance setup at 3:00 AM")
    print("Bot will keep running to handle scheduled tasks...")
    print("Press Ctrl+C to stop")
    
    try:
        while True:
            schedule.run_pending()
            time.sleep(60)
    except KeyboardInterrupt:
        print("\nStopped by user")