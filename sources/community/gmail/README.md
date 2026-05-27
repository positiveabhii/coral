# Google Gmail (Community)

**Version:** 0.1.0
**Backend:** HTTP (Google Gmail v1 REST API)
**Tables:** 2
**Base URL:** `https://gmail.googleapis.com/gmail/v1/users/me`

Query messages and conversation threads directly from your Google Gmail mailbox via SQL. 
Designed for workspace interaction logging, communication relationship mapping, and automated
context indexing. Pairs naturally with development tracking engines like the **GitHub** source 
for unified engineering analytics dashboards.

## Setup

### 1. Create a Google Cloud Console Project

1. Navigate to the [Google Cloud Console](https://console.cloud.google.com/).
2. Create a new project and enable the **Gmail API** under **APIs & Services → Enabled APIs**.
3. Configure your **OAuth Consent Screen** (Internal or External) and add the following scopes:
   * `https://www.googleapis.com/auth/gmail.readonly`
   * `https://www.googleapis.com/auth/gmail.send`
4. Go to **Credentials → Create Credentials → OAuth client ID**.
5. Select **Web Application** as the application type.
6. Add the following authorized redirect URI exactly as shown:
