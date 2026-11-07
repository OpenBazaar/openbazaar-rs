import { Platform } from "react-native";
import * as Notifications from "expo-notifications";
import * as TaskManager from "expo-task-manager";

import { disablePush, enablePush } from "./remoteApi.ts";
import { boot, getSession, setPushEnabled, sync } from "./session.ts";
import type { SyncChanges } from "../homeNode/replica.ts";

// Push is opt-in and content-free: the node sends a wake-up through the
// owner's push gateway, the app syncs, and any notice it shows is generic.

export const WAKE_TASK = "ob-home-wake";

Notifications.setNotificationHandler({
  handleNotification: async () => ({ shouldShowBanner: true, shouldShowList: true, shouldPlaySound: false, shouldSetBadge: false }),
});

export function hasNewOrders(changes: SyncChanges | null): boolean {
  return !!changes?.added.orders?.length;
}

export async function notifyGenerically(): Promise<void> {
  const permission = await Notifications.getPermissionsAsync();
  if (!permission.granted) return;
  await Notifications.scheduleNotificationAsync({ content: { title: "OpenBazaar", body: "Something changed in your store" }, trigger: null });
}

/** Ask for notification permission (Android 13+ and iOS). */
export async function allowNotifications(): Promise<boolean> {
  const current = await Notifications.getPermissionsAsync();
  if (current.granted) return true;
  return (await Notifications.requestPermissionsAsync()).granted;
}

export async function turnPushOn(): Promise<void> {
  if (!(await allowNotifications())) throw new Error("Allow notifications for OpenBazaar in Settings first.");
  let token: Notifications.DevicePushToken;
  try {
    token = await Notifications.getDevicePushTokenAsync();
  } catch {
    throw new Error(
      Platform.OS === "ios"
        ? "This build has no push entitlement. Push needs an App Store or signed build."
        : "This build has no Firebase configuration, so it cannot receive push. Background checks still run.",
    );
  }
  await enablePush(Platform.OS === "ios" ? "apns" : "fcm", String(token.data));
  await setPushEnabled(true);
  await Notifications.registerTaskAsync(WAKE_TASK).catch(() => null);
}

export async function turnPushOff(): Promise<void> {
  await disablePush();
  await setPushEnabled(false);
  await Notifications.unregisterTaskAsync(WAKE_TASK).catch(() => null);
}

async function onWake(): Promise<void> {
  if (getSession().status !== "paired") await boot();
  const session = getSession();
  if (!session.pairing?.pushEnabled) return;
  const changes = await sync({ watch: true });
  if (changes && (hasNewOrders(changes) || Object.keys(changes.added).length || Object.keys(changes.changed).length)) await notifyGenerically();
}

TaskManager.defineTask(WAKE_TASK, async () => {
  await onWake().catch(() => null);
});

/** A wake-up that arrives while the app is open: sync, no notice. */
export function listenForWakes(): () => void {
  const subscription = Notifications.addNotificationReceivedListener(() => {
    void sync();
  });
  return () => subscription.remove();
}
