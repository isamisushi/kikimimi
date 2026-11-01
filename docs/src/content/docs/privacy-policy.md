---
title: Privacy Policy
description: How kikimimi handles account information, Google sign-in data, activity metadata, and optional Cloud and Router services.
---

Last updated: October 27, 2026

This policy describes how **kikimimi** ("we", "us") handles information when you use kikimimi.dev, Kikimimi Cloud, and our optional connected services. Contact us at [kikimimi-support@googlegroups.com](mailto:kikimimi-support@googlegroups.com) with privacy questions or requests.

## Local use and optional Cloud services

The open-source CLI, desktop app, and local dashboard can operate without a Cloud account. Activity records are stored locally. Connecting to Cloud enables uploads of activity metadata. If you configure your own S3 storage, that storage is controlled by you or your organization. Uninstalling the local app does not delete records already uploaded to Cloud or your S3 storage.

The optional **Router** is a separate inference service: requests you deliberately send through it include prompts and other request content. The activity collector's metadata-only design does not mean that Router inference requests contain no content.

## Information we access and collect

- **Account and sign-in information:** your email address, authentication-provider identifiers, verification status, and account, session, and workspace records. Email-link sign-in also processes your email address and a temporary sign-in token.
- **Google sign-in:** we request `openid`, `email`, and `profile`. Google may return basic profile information, including a name and profile picture. Our current sign-in handler uses your Google account identifier (`sub`), email address, and email-verification status; it does not store your Google name or profile picture. It uses a Google access token during sign-in to retrieve this information and does not persist that token or request offline access. We do not request access to Gmail messages, Google Drive files, Calendar, or Contacts.
- **GitHub sign-in:** your GitHub account identifier and verified email address used to identify your kikimimi account.
- **Cloud activity metadata:** tool, MCP server, skill and model names; timestamps and durations; success, failure and permission outcomes; token counts and reported costs; and device, session, account, and repository identifiers. Repository identifiers and other metadata can reveal information about your work even though they are not file contents.
- **Workspace and billing information:** membership and roles, invitations, team configuration, information supplied for billing and invoices, payment status, and tool inventory or subscription details that you or a workspace administrator enter.
- **Router information, if you use it:** prompts, messages, and other content included in inference requests are processed to provide model responses. Usage records include account/workspace attribution, models, token counts, cost, and timing. Prompt and response logging is disabled in the Router's spend logs; this is not a promise that upstream providers retain no data.
- **Support and service operations:** information you send to support and operational request information, such as IP addresses and error or security logs, that our service and infrastructure providers process to deliver and protect the service.

The activity collectors do not populate prompt, tool-argument, file-content, or tool-output fields in uploaded activity events. Cloud ingestion also discards the reserved event-body fields. See [Activity data and privacy controls](/kikimimi/privacy/) for the technical details.

## How we use information

We use information to authenticate you, maintain sessions, associate sign-in methods with an account, provide dashboards and exports, manage workspace access, process requested inference, administer billing, respond to support requests, and investigate service failures or abuse. Sign-in methods with the same verified email may be associated with an existing account.

We also use limited onboarding records and aggregate usage information to understand whether setup and Cloud synchronization work. Onboarding records identify setup steps and their first occurrence; they are not additional prompt or file collection.

**Google user data is used for account access and related service operations.** We do not sell Google user data, use it for advertising, or use it to train generalized AI or machine-learning models. Kikimimi's use and transfer of information received from Google APIs adheres to the [Google API Services User Data Policy](https://developers.google.com/terms/api-services-user-data-policy), including its Limited Use requirements where applicable.

## Sharing and service providers

We share information only as needed to provide the service, follow your instructions, protect the service and its users, or comply with applicable legal obligations:

- **Your workspace:** authorized members and administrators can access information according to their roles. Administrators may see member identifiers, usage, and permitted activity details. Only connect data you are authorized to share with that workspace.
- **Infrastructure and communications:** Fly.io hosts our Cloud services. Hosting, database, and email service providers process information needed to operate the service. Support messages sent to our Google Groups address are processed by Google and accessible to the support group's authorized members.
- **Authentication providers:** Google or GitHub processes the sign-in you initiate under its own policies.
- **Model providers:** when you use Router, the selected model provider and any intermediary, such as OpenRouter, receive the inference content needed to handle your request. Their terms and retention policies apply to their processing. Your Google sign-in profile is not supplied as model input by our authentication flow.
- **Legal and security requests:** we may disclose necessary information in response to a valid legal obligation or to investigate fraud, abuse, or threats to the service.

Service providers may process information in countries other than your own. Local-only activity files and your own S3 storage remain under your control unless you choose to share them.

## Storage, security, and retention

Cloud account and activity information is stored in service databases. We use HTTPS for public service connections, access controls, scoped credentials, and workspace authorization checks to protect information. No service can guarantee absolute security.

Account and uploaded activity records are retained while needed to provide your account and service, unless you request deletion. There is currently no published automatic expiry period for Cloud activity history. Billing, security, and dispute-related records may need to be retained after account closure for their applicable purpose or legal requirements. Backups may retain copies until they are replaced or expire; deletion does not necessarily remove every backup copy immediately.

## Cookies and local storage

We use cookies and browser storage for sign-in, session security, workspace selection, and interface preferences. Blocking required session cookies prevents Cloud login. These mechanisms are not used to build advertising profiles.

## Your choices and deletion requests

You can use the local app without Cloud, control which repositories are shared with a team, export Cloud activity, and stop future uploads by disconnecting Cloud on your devices. Stopping uploads or removing an app does not itself erase existing Cloud records.

For access, correction, export assistance, account closure, or deletion of your Cloud data, email [kikimimi-support@googlegroups.com](mailto:kikimimi-support@googlegroups.com) from your account email address. Identify the account or workspace concerned; do not send passwords, API keys, or OAuth secrets. We may verify your identity and authority before acting. We will explain any records that cannot be deleted because of legal obligations or another workspace's rights.

You can also remove kikimimi's connection from your [Google Account connections](https://myaccount.google.com/connections). Removing Google's authorization does not by itself delete your kikimimi account, end all existing kikimimi sessions, or delete previously stored data; contact us for account deletion. Requests about organization-managed information may require coordination with your workspace administrator. Local files and customer-managed S3 copies must be removed by their owner.

## Changes to this policy

We will update this page and its revision date when our practices change. For material changes, we will provide an appropriate notice. If a new use of Google data requires additional consent, we will request it before that use.

See also our [Terms of Service](/kikimimi/terms/).
