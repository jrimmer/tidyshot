#pragma once

// Queue a graceful application quit onto the Qt main thread.
//
// A forwarded command arrives on the single-instance socket thread;
// QCoreApplication::quit() must run on the thread that owns the event loop, so
// this posts it rather than exiting the process from the worker.
void tidyshot_request_quit();
