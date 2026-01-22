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
    
    # Additional stability preferences
    firefox_options.set_preference("dom.webdriver.enabled", False)
    firefox_options.set_preference("useAutomationExtension", False)
    firefox_options.set_preference("dom.disable_beforeunload", True)
    firefox_options.set_preference("browser.tabs.remote.autostart", False)
    firefox_options.set_preference("browser.tabs.remote.autostart.2", False)

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
    
    try:
        print(f"Navigating to: {OLE_URL}")
        driver.get(OLE_URL)
        print(f"Navigation completed. Current URL: {driver.current_url}")
        print(f"Page title: {driver.title}")
        
        # Check if we got redirected or if there's an issue
        if "404" in driver.title.lower() or "error" in driver.title.lower():
            print(f"Warning: Page title suggests an error: {driver.title}")
        
        # Wait for login page to load and find userid element
        print("Waiting for login page to load...")
        userid_element = WebDriverWait(driver, 15).until(
            lambda d: d.find_element("id", "userid")
        )
        print("Login page loaded successfully")

        # Login
        driver.find_element("id", "userid").send_keys(username)
        driver.find_element("id", "pwd").send_keys(password)
        driver.find_element("name", "loginButton2").click()
        print("Login credentials submitted")
        
    except TimeoutException:
        print("Error: Login page did not load properly - userid element not found")
        print(f"Current URL: {driver.current_url}")
        print("Page title:", driver.title if driver.title else "No title")
        # Save page source for debugging
        try:
            with open('/tmp/login_page_debug.html', 'w', encoding='utf-8') as f:
                f.write(driver.page_source)
            print("Page source saved to /tmp/login_page_debug.html for debugging")
        except:
            print("Could not save page source")
        driver.quit()
        raise
    except Exception as e:
        print(f"Error during login process: {e}")
        print(f"Current URL: {driver.current_url}")
        driver.quit()
        raise

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

def filter_today_classes(json_data):
    """Filter classes to only include those scheduled for today"""
    if not json_data:
        print("No JSON data to filter")
        return json_data
    
    # If the result is not successful, return as-is
    if json_data.get("result") != 1:
        print(f"API result is not successful (result: {json_data.get('result')}), returning unfiltered data")
        return json_data
    
    today_date = get_hk_time().date()
    print(f"Filtering classes for today's date: {today_date}")
    
    filtered_classes = []
    original_count = 0
    filtered_count = 0
    
    classes_list = json_data.get("classes", [])
    if not classes_list:
        print("No classes found in the response")
        return json_data
    
    for course in classes_list:
        filtered_course = {
            "termcode": course.get("termcode", ""),
            "course_code": course.get("course_code", ""),
            "classes": []
        }
        
        for class_session in course.get("classes", []):
            original_count += 1
            datetime_str = class_session.get("datetime", "")
            
            if datetime_str:
                class_datetime = parse_class_time(datetime_str)

                if class_datetime:
                    class_date = class_datetime.date()
                    
                    # is the class today
                    if class_date == today_date:
                        filtered_course["classes"].append(class_session)
                        filtered_count += 1
                        print(f"Keeping class: {course.get('course_code')} - {class_session.get('name')} at {datetime_str}")
                    else:
                        print(f"Filtering out class: {course.get('course_code')} - {class_session.get('name')} at {datetime_str} (not today)")
                else:
                    print(f"Warning: Could not parse datetime for class: {course.get('course_code')} - {datetime_str}")
            else:
                print(f"Warning: No datetime found for class: {course.get('course_code')} - {class_session.get('name')}")
        
        if filtered_course["classes"]:
            filtered_classes.append(filtered_course)
    
    print(f"Filtered classes: {filtered_count}/{original_count} classes are for today")
    
    # Return the filtered data with the same structure
    filtered_data = {
        "result": json_data.get("result", 0),
        "classes": filtered_classes
    }
    
    return filtered_data

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
                
                # Filter classes to only include today's classes
                filtered_data = filter_today_classes(json_data)
                return filtered_data
                
            except json.JSONDecodeError:
                print(f"Response Body (not JSON): {body}")
    else:
        print("ERROR!!!! No getTodayClass request found!")

def send_classes(classes, retries=0):
    """Send Discord webhook notification about today's classes"""
    # Classes are already filtered for today by filter_today_classes function
    if classes and classes.get("result") == 1:
        class_list = classes.get("classes", [])
        if class_list:
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
                    
                    # Format time nicely
                    if datetime_str and datetime_str != "Unknown Time":
                        try:
                            start_time = datetime_str.split(" ")[1]  # Get time part
                            end_time = endtime.split(" ")[1] if endtime else ""
                            time_range = f"{start_time} - {end_time}" if end_time else start_time
                            
                            message += f"> **{course_code}** - {class_name}\n"
                            message += f">  TIME: {time_range}\n"
                            message += f">  VENUE: {venue}"
                            if group:
                                message += f" ({group})"
                            message += "\n"
                            
                        except Exception as e:
                            print(f"Error processing class {course_code} for Discord: {e}")
                            continue
        else:
            message = "No classes scheduled for today!"
    else:
        # 3 tries
        if retries <= 3:
            print(f"Retrying fetching classes in 30 seconds, attempt {retries + 1}")
            time.sleep(30)
            daily_attendance_task(retries + 1)
            return
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
    # Classes are already filtered for today by filter_today_classes function
    
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
                            "url": f"https://iole.hkmu.edu.hk/course{termcode}/{course_code}.nsf//class_activities_student?readform&"
                        }
                        
                        scheduled_classes.append(class_info)
                        print(f"Scheduled: {course_code} - {class_name} at {class_datetime}")
                        
                    except ValueError as e:
                        print(f"Error parsing datetime for {course_code}: {e}")
    
    print(f"\nTotal scheduled classes: {len(scheduled_classes)}")
    print(f"Current System Time (HKT): {get_hk_time().strftime('%Y-%m-%d %H:%M:%S %Z')}")
    
    # Schedule attendance for each class
    for class_info in scheduled_classes:
        schedule_class_attendance(class_info, username, password)

def schedule_class_attendance(class_info, username, password):
    """Schedule attendance marking for a specific class"""
    class_time = class_info["datetime"]
    class_endtime = class_info.get("endtime")
    current_time = get_hk_time()
    
    print(f"System Time: {current_time.strftime('%Y-%m-%d %H:%M:%S %Z')}")
    print(f"Class Time: {class_time.strftime('%Y-%m-%d %H:%M:%S %Z') if hasattr(class_time, 'tzinfo') and class_time.tzinfo else class_time.strftime('%Y-%m-%d %H:%M:%S')}")
    
    # If no endtime specified, default to 3 hours after class start
    if not class_endtime:
        class_endtime = class_time + timedelta(hours=3)
        print(f"Class End Time (estimated): {class_endtime.strftime('%Y-%m-%d %H:%M:%S %Z')}")
    else:
        print(f"Class End Time: {class_endtime.strftime('%Y-%m-%d %H:%M:%S %Z') if hasattr(class_endtime, 'tzinfo') and class_endtime.tzinfo else class_endtime.strftime('%Y-%m-%d %H:%M:%S')}")
    
    # Calculate delay until class starts
    delay = (class_time - current_time).total_seconds()
    
    if delay > 0:
        print(f"Scheduling attendance for {class_info['course_code']} in {delay/60:.1f} minutes")
        
        # Schedule the attendance marking
        timer = threading.Timer(delay, mark_attendance, args=[class_info, username, password])
        timer.daemon = True
        timer.start()
    elif current_time <= class_endtime:
        # Class has started but hasn't ended yet - attempt attendance immediately
        minutes_since_start = (current_time - class_time).total_seconds() / 60
        minutes_until_end = (class_endtime - current_time).total_seconds() / 60
        print(f"Class {class_info['course_code']} is IN PROGRESS!")
        print(f"Started {minutes_since_start:.0f} minutes ago, ends in {minutes_until_end:.0f} minutes")
        print(f"Attempting attendance immediately...")
        
        # Start attendance marking immediately in a separate thread
        attendance_thread = threading.Thread(target=mark_attendance, args=[class_info, username, password])
        attendance_thread.daemon = True
        attendance_thread.start()
    else:
        # Class has already ended
        minutes_since_end = (current_time - class_endtime).total_seconds() / 60
        print(f"Class {class_info['course_code']} has already ENDED {minutes_since_end:.0f} minutes ago")

def mark_attendance(class_info, username, password):
    """Mark attendance for a specific class"""
    print(f"\nStarting attendance for {class_info['course_code']} - {class_info['class_name']}")
    print(f"Attendance Start Time: {get_hk_time().strftime('%Y-%m-%d %H:%M:%S %Z')}")
    
    # Create new driver session and login to OLE
    driver = None
    try:
        driver = enter_ole(username, password)
        
        class_url = class_info["url"]
        print(f"Navigating to: {class_url}")
        driver.get(class_url)
        
        # Wait for page to load before checking for attendance elements
        print("Waiting for page to load...")
        time.sleep(15)  # Wait 15 seconds for page to fully load
        
        # Try to find submitted_msg element every 10 minutes until class endtime
        current_time = get_hk_time()
        class_endtime = class_info.get("endtime")
        
        # If no endtime specified, default to 3 hours after class start
        if not class_endtime:
            class_endtime = class_info["datetime"] + timedelta(hours=3)
        
        print(f"Will check for attendance submission until: {class_endtime}")
        print(f"Current Time: {current_time.strftime('%Y-%m-%d %H:%M:%S %Z')}")
        attempt = 0
        warning_sent = False  # Track if we've sent the 30-minute warning
        
        while current_time < class_endtime:
            try:
                print(f"Attempt {attempt + 1}: Checking if attendance has been submitted...")
                
                # Wait a moment for any dynamic content to load
                time.sleep(3)  # Wait 3 seconds before checking elements
                
                # Check if the submitted_msg element exists on the page
                try:
                    submitted_element = driver.find_element("id", "submitted_msg")
                    print(f"Found attendance confirmation element (submitted_msg)!")
                    
                    # Send success notification
                    success_message = f"**- Attendance Confirmed!**\n"
                    success_message += f"> Course: {class_info['course_code']}\n"
                    success_message += f"> Class: {class_info['class_name']}\n"
                    success_message += f"> Time: {get_hk_time().strftime('%Y-%m-%d %H:%M:%S %Z')}\n"
                    success_message += f"> Status: Attendance successfully submitted"
                    
                    send_discord_notification(success_message)
                    
                    print(f"Attendance confirmed for {class_info['course_code']}")
                    break
                    
                except NoSuchElementException:
                    print(f"Attendance not yet submitted (submitted_msg element not found)")
                # Check if we're in the last 30 minutes and haven't sent warning yet
                time_remaining = (class_endtime - current_time).total_seconds()
                minutes_remaining = time_remaining / 60
                
                if minutes_remaining <= 30 and not warning_sent:
                    print(f"- WARNING: Only {minutes_remaining:.0f} minutes left in class!")
                    
                    # Send warning notification
                    warning_message = f"**- ATTENDANCE WARNING**\n"
                    warning_message += f"> Course: {class_info['course_code']}\n"
                    warning_message += f"> Class: {class_info['class_name']}\n"
                    warning_message += f"> Only {minutes_remaining:.0f} minutes remaining!\n"
                    warning_message += f"> Attempts so far: {attempt + 1}"
                    
                    send_discord_notification(warning_message)
                    warning_sent = True
                
                print(f"Attendance not submitted yet, waiting 10 minutes... ({minutes_remaining:.0f} minutes left)")
                time.sleep(600)  # Wait 10 minutes
                driver.refresh()  # Refresh the page
                print("Page refreshed, waiting for reload...")
                time.sleep(10)  # Wait 10 seconds after refresh for page to reload
                attempt += 1
                current_time = get_hk_time()  # Update current time
                    
            except Exception as e:
                print(f"Error during attendance attempt {attempt + 1}: {e}")
                
                # Check for warning even during errors
                time_remaining = (class_endtime - current_time).total_seconds()
                minutes_remaining = time_remaining / 60
                
                if minutes_remaining <= 30 and not warning_sent:
                    print(f"- WARNING: Only {minutes_remaining:.0f} minutes left in class!")
                    
                    # Send warning notification
                    warning_message = f"**- ATTENDANCE WARNING**\n"
                    warning_message += f"> Course: {class_info['course_code']}\n"
                    warning_message += f"> Class: {class_info['class_name']}\n"
                    warning_message += f"> Only {minutes_remaining:.0f} minutes remaining!\n"
                    warning_message += f"ERROR occurred during attempt {attempt + 1}\n"
                    warning_message += f"Total attempts: {attempt + 1}"
                    
                    send_discord_notification(warning_message)
                    warning_sent = True
                
                attempt += 1
                time.sleep(600)  # Wait 10 minutes before retry
                current_time = get_hk_time()  # Update current time
        
        # Send failure notification if time exceeded without finding submitted_msg
        if current_time >= class_endtime:
            fail_message = f"**- Attendance FAILED**\n"
            fail_message += f"> Course: {class_info['course_code']}\n"
            fail_message += f"> Status: Attendance submission not confirmed by end of class\n"
            fail_message += f"> Total attempts: {attempt}"
            
            send_discord_notification(fail_message)
            
    except Exception as e:
        print(f"Error marking attendance for {class_info['course_code']}: {e}")
        
        error_message = f"**- Attendance ERROR**\n"
        error_message += f"> Course: {class_info['course_code']}\n"
        error_message += f"`Error: {str(e)}`"
        
        send_discord_notification(error_message)
    
    finally:
        if driver:
            try:
                driver.quit()
                print(f"Driver cleaned up for {class_info['course_code']}")
            except Exception as e:
                print(f"Error cleaning up driver: {e}")



def daily_attendance_task(retries=0):
    """Main function that runs daily at 3 AM to set up attendance for the day"""
    if not STUDENT_ID or not STUDENT_PASSWORD:
        error_msg = "MISSING required environment variables: STUDENT_ID and/or STUDENT_PASSWORD"
        print(error_msg)
        send_discord_notification(f"**Configuration Error**\n{error_msg}")
        return
    
    if not DISCORD_WEBHOOK:
        print("Warning: DISCORD_WEBHOOK not set, notifications disabled")

    try:
        # Create initial driver session for getting classes with retry logic
        print("- Creating initial driver session...")
        initial_driver = None
        max_retries = 3
        
        for attempt in range(max_retries):
            try:
                print(f"  Login attempt {attempt + 1}/{max_retries}")
                initial_driver = enter_ole(STUDENT_ID, STUDENT_PASSWORD)
                print("  Login successful!")
                break
            except Exception as login_error:
                print(f"  Login attempt {attempt + 1} failed: {login_error}")
                if attempt == max_retries - 1:
                    raise login_error
                print(f"  Retrying in 10 seconds...")
                time.sleep(10)

        # get json data for classes
        print("- Retrieving today's classes...")
        classes = getTodayClasses(initial_driver, "https://oleconnect.hkmu.edu.hk/oledb/api/getTodayClass/")
        
        # Close initial driver as we're done with it
        initial_driver.quit()
        print("- Initial driver session closed")
        
        # Send classes to Discord
        print("- Sending class notification to Discord...")
        send_classes(classes, retries)

        # Schedule
        print("- Scheduling attendance for today's classes...")
        schedule_attendance(classes, STUDENT_ID, STUDENT_PASSWORD)
        
    except Exception as e:
        print(f"- ERROR during daily setup: {e}")
        
        error_message = f"**Daily Setup Error**\n"
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