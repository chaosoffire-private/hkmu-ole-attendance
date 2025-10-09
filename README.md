# HKMU Attendance Bot

Automated attendance monitoring system for Hong Kong Metropolitan University (HKMU) using Selenium and Discord notifications.

## Features

- 🕒 **Daily Scheduling**: Runs automatically at 3:00 AM every day
- 🚀 **Immediate Start**: Also runs once on startup
- 📱 **Discord Notifications**: Real-time updates on class schedules and attendance status
- ⚠️ **Smart Warnings**: 30-minute warnings if attendance isn't available yet
- 🔄 **Session Management**: Fresh browser sessions for each operation
- 🐳 **Docker Support**: Easy deployment with Docker

## Docker Deployment

### Quick Start with Docker Compose

1. **Clone the repository**:
   ```bash
   git clone <your-repo-url>
   cd hkmu-attendance
   ```

2. **Build and run with Docker Compose**:
   ```bash
   docker-compose up -d
   ```

3. **View logs**:
   ```bash
   docker-compose logs -f
   ```

4. **Stop the bot**:
   ```bash
   docker-compose down
   ```

### Manual Docker Build

1. **Build the image**:
   ```bash
   docker build -t hkmu-attendance .
   ```

2. **Run the container**:
   ```bash
   docker run -d \
     --name hkmu-attendance-bot \
     --restart unless-stopped \
     -e TZ=Asia/Hong_Kong \
     hkmu-attendance
   ```

## Configuration

### Environment Variables

- `TZ`: Timezone (default: `Asia/Hong_Kong`)
- `PYTHONUNBUFFERED`: Ensures output is shown in real-time

### Discord Webhook

Update the `DISCORD_WEBHOOK` variable in `main.py` with your Discord webhook URL.

### User Credentials

Update the user credentials in the `daily_attendance_task()` function:
```python
user = "your_student_id"
pwd = "your_password"
```

## Local Development

1. **Install Python dependencies**:
   ```bash
   pip install -r requirements.txt
   ```

2. **Install Firefox** (required for Selenium)

3. **Run the bot**:
   ```bash
   python main.py
   ```

## How It Works

1. **Daily Setup (3:00 AM)**:
   - Logs into HKMU OLE system
   - Retrieves today's class schedule
   - Sends class summary to Discord
   - Schedules attendance checking for each class

2. **Attendance Monitoring**:
   - At each class start time, creates new browser session
   - Navigates to class attendance page
   - Checks every 10 minutes for "present" word on page
   - Sends Discord notifications for success/failure/warnings

3. **Smart Features**:
   - 30-minute warning if attendance not available
   - Automatic retry until class end time
   - Proper browser session cleanup
   - Error handling and notifications

## Docker Architecture

- **Base Image**: Python 3.11 slim
- **Browser**: Firefox ESR (headless mode)
- **Display**: Virtual framebuffer (Xvfb)
- **Security**: Non-root user execution
- **Resources**: Configurable CPU/memory limits

## Monitoring

### Health Check
The Docker container includes a health check that monitors the Python process.

### Logs
View real-time logs:
```bash
# Docker Compose
docker-compose logs -f

# Docker
docker logs -f hkmu-attendance-bot
```

## Troubleshooting

### Common Issues

1. **Firefox not starting**: Ensure Xvfb is running and DISPLAY is set
2. **Network issues**: Check if the container can access external URLs
3. **Memory issues**: Increase memory limits in docker-compose.yml

### Debug Mode
To run with more verbose output, modify the Dockerfile CMD:
```dockerfile
CMD ["sh", "-c", "Xvfb :99 -screen 0 1024x768x24 > /dev/null 2>&1 & python -u main.py"]
```

## Security Notes

- Credentials are stored in the source code (consider using environment variables)
- Discord webhook URL is exposed (consider using secrets management)
- Running as non-root user in container for security

## License

[Add your license here]