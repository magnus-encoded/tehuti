# 9. The user grants, per application

## Status

Accepted

## Context

Plugins can read things from the user, starting with part of an application's window. Installing a plugin is not enough for that.

## Decision

A plugin learns something about the user only after the user has said yes, for one named application, to a question that Tehuti itself asks. Tehuti enforces the limit: the plugin never receives more than was granted. The user can see every grant and take it back.

No grant covers all applications.
