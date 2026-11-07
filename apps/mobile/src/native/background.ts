import { Platform } from "react-native";
import * as BackgroundTask from "expo-background-task";
import * as TaskManager from "expo-task-manager";

import { waitForTor } from "./channels.ts";
import { hasNewOrders, notifyGenerically } from "./push.ts";
import { boot, getSession, sync } from "./session.ts";

// Android only: every 15 minutes or more (the OS decides), start Tor, sync
// with the watch key, and post a generic notice if a new order arrived.
// iOS syncs when the app opens, and on a push wake-up if push is on.

export const CHECK_TASK = "ob-home-check";

TaskManager.defineTask(CHECK_TASK, async () => {
  try {
    if (getSession().status !== "paired") await boot();
    if (getSession().status !== "paired") return BackgroundTask.BackgroundTaskResult.Success;
    await waitForTor(90_000);
    const changes = await sync({ watch: true });
    if (hasNewOrders(changes)) await notifyGenerically();
    return BackgroundTask.BackgroundTaskResult.Success;
  } catch {
    return BackgroundTask.BackgroundTaskResult.Failed;
  }
});

export async function scheduleBackgroundChecks(): Promise<void> {
  if (Platform.OS !== "android") return;
  if (await TaskManager.isTaskRegisteredAsync(CHECK_TASK)) return;
  await BackgroundTask.registerTaskAsync(CHECK_TASK, { minimumInterval: 15 });
}

export async function stopBackgroundChecks(): Promise<void> {
  if (await TaskManager.isTaskRegisteredAsync(CHECK_TASK)) await BackgroundTask.unregisterTaskAsync(CHECK_TASK);
}
