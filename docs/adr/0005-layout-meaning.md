# 5. Layout meaning

## Status

Accepted

## Context

Position on the screen carries meaning. One rule places every layer and every grouping, so new features are placed the same way and layouts stay consistent.

## Decision

The screen is an axis from the computer at the top to the human at the bottom. What the computer can track sits higher. What only the human's idea can define sits lower. Intent travels upward.

Controls, bottom to top: bar with the powerline, draft row, keys of the on-screen keyboard, application. The user speaks or moves the controller below the screen, and intent rises through them. The application passes it on to the operating system, the kernel and the hardware.

Content follows the same gradient. Columns, ordered by when windows opened, are computer-tracked and sit on the first row. A workspace groups windows by what the user thinks ties them together, so workspaces sit below the columns.

## Consequences

- This is hard to reverse. Layouts, the bar, the keyboard and workspaces all depend on the direction.
- Application content stays on top. Input and status stay low, near the human.
- New features go where their directness to the human puts them: the more the human defines it, the lower it goes.
- A layout breaks this rule when a human-facing control sits above the content it acts on, when the application covers the controls that steer it, or when a human-defined grouping sits above a computer-tracked order.
- Layout by frequency of use is rejected. The bar, the keyboard and workspaces depend on a fixed direction, so an ordering that reshuffles them can't be used.
