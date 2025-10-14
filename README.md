# hkmu-ole-attendance
Personal use for taking attendance for Hong Kong Metropolitan University's Online Learning Environment System.

> Might open-source it, but prefer keeping it quiet.

This will NOT work for lectures or tutorials that are using IBC's iAttend
as it will require bluetooth signal in which this attendance system is unable to achieve.

For my course, most professors uses the class activities attendance system.

## How it works
You only need to run this program using docker and does not require inbound ports.

The program will automatically login to your OLE every day at 3AM HKT and attempt to capture the upcoming classes that day.
```json
{
    "result": 1,
    "classes": [
        {
            "termcode": "2504",
            "course_code": "COMP3120SEF",
            "classes": [
                {
                    "name": "PC Laboratory ( Full Time )",
                    "datetime": "2025-10-15 16:00",
                    "endtime": "2025-10-15 16:50",
                    "group": "P02",
                    "venue": "JCPC D0627",
                    "host": "kwtse"
                }
            ]
        },
        {
            "termcode": "2504",
            "course_code": "COMP3200SEF",
            "classes": [
                {
                    "name": "Lecture ( Full Time )",
                    "datetime": "2025-10-16 09:00",
                    "endtime": "2025-10-16 10:50",
                    "group": "L01",
                    "venue": "IOH F0201",
                    "host": "thluk"
                },
                {
                    "name": "PC Laboratory ( Full Time )",
                    "datetime": "2025-10-15 15:00",
                    "endtime": "2025-10-15 15:50",
                    "group": "P02",
                    "venue": "JCPC D0626",
                    "host": "thluk"
                }
            ]
        },
        {
            "termcode": "2504",
            "course_code": "COMP3500SEF",
            "classes": [
                {
                    "name": "Lecture ( Full Time )",
                    "datetime": "2025-10-14 09:00",
                    "endtime": "2025-10-14 10:50",
                    "group": "L01",
                    "venue": "JCC D0212",
                    "host": "nezeamuz"
                },
                {
                    "name": "Tutorial ( Full Time )",
                    "datetime": "2025-10-14 11:00",
                    "endtime": "2025-10-14 11:50",
                    "group": "T01",
                    "venue": "JCC D0212",
                    "host": "nezeamuz"
                }
            ]
        },
        {
            "termcode": "2504",
            "course_code": "COMP3810SEF",
            "classes": [
                {
                    "name": "Lecture ( Full Time )",
                    "datetime": "2025-10-14 14:00",
                    "endtime": "2025-10-14 15:50",
                    "group": "L01",
                    "venue": "JCC D0212",
                    "host": "sliy"
                }
            ]
        }
    ],
    "is_cc": false
}
```

Once the class has started according to `datetime`, it will attempt class attendance every 10 minutes till the `endtime`
![working-example](https://h5ai.avanlcy.hk/temp/hkmu-ole-attendance-1.png)

## Optional: Discord notification
Optionally, you can setup a discord webhook to notify yourself when the program attempts, sucessfully or failed an attendance.
Simply go to a channel: `Edit Channel > Integrations > Webhooks > New Webhook > Copy Webhook URL`
![discord-example](https://h5ai.avanlcy.hk/temp/hkmu-ole-attendance-2.png)

If an attendance has completed successfully, you will receive the notification:
![discord-example-2](https://h5ai.avanlcy.hk/temp/hkmu-ole-attendance-3.png) 

If the program is unable to confirm if has successfully attended the class, you will receive a warning 30 minutes before the `endtime`
![discord-example-3](https://h5ai.avanlcy.hk/temp/hkmu-ole-attendance-4.png)
