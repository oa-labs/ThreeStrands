# Date display

Use `src/dateFormatting.ts` for general dates. Let `Intl.DateTimeFormat` use
the device locale and local timezone; examples below assume US English.

- Compact lists and exact history dates: **Sep 5** in the current local year;
  **Sep 5, 2025** otherwise, including future years. Keep the day when adding
  the year. Mail uses the time for today because recency within the day matters.
- Filter boundaries and standalone date facts: **Sep 5, 2026**. Always include
  the year so a saved or restored filter stays explicit. Parse date-only filter
  values as local calendar dates, never UTC midnight.
- Metadata and detailed timestamps: **Sep 5, 2026, 3:04 PM**. Include the time
  only when the instant matters (sent, received, added, edited, or deleted).
- Operational sync and AI diagnostics retain seconds to distinguish events within
  the same minute; their date uses the same full-date style.
- Relationship-age aggregates: **Mar 2025** in facts such as “emails since”.
  These summarize a period; individual messages keep their exact day.

Intentional specialized formats:

- Today/Calendar headings, daily note titles, and picker accessibility labels
  use longer dates and weekdays for orientation. Calendar grids may omit a
  repeated month/year when their heading supplies it.
- Task and upcoming-event lists use Today/Tomorrow/Yesterday or a nearby
  weekday where useful, then fall back to the shared compact date.
- Event times, proposed slots, and scheduled sending use the event or selected
  timezone. All-day/date-only values remain calendar dates, without an invented
  time. Birthdays may omit an unknown birth year.
- Machine keys, provider requests, input values, and exported correspondence
  have their own serialization requirements; display formatting must not change
  them.

Avoid numeric-only dates in general display labels and padded days in compact
lists. Do not drop the day from an older exact date just to add its year.
