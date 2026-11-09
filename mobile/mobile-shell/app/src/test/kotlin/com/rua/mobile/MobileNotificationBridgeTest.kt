package com.rua.mobile

import android.content.Intent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.RuntimeEnvironment

@RunWith(RobolectricTestRunner::class)
class MobileNotificationBridgeTest {
    @Test
    fun missingPermissionRequestsOnceAndPresentsLatestNotificationAfterGrant() {
        var granted = false
        var requests = 0
        var permissionResult: ((Boolean) -> Unit)? = null
        val presented = mutableListOf<MobileUserNotification>()
        val gate = NotificationPermissionGate(
            hasPermission = { granted },
            requestPermission = { callback ->
                requests += 1
                permissionResult = callback
            },
            present = presented::add,
        )
        val first = notification("First")
        val latest = notification("Latest")

        assertTrue(gate.submit(first))
        assertTrue(gate.submit(latest))
        assertEquals(1, requests)
        assertTrue(presented.isEmpty())

        granted = true
        permissionResult?.invoke(true)

        assertEquals(listOf(latest), presented)
    }

    @Test
    fun deniedPermissionDropsPendingNotification() {
        var permissionResult: ((Boolean) -> Unit)? = null
        val presented = mutableListOf<MobileUserNotification>()
        val gate = NotificationPermissionGate(
            hasPermission = { false },
            requestPermission = { callback -> permissionResult = callback },
            present = presented::add,
        )

        assertTrue(gate.submit(notification("Denied")))
        permissionResult?.invoke(false)

        assertTrue(presented.isEmpty())
    }

    @Test
    fun bridgeAdvertisesExplicitNotificationCapability() {
        val activity = org.robolectric.Robolectric.buildActivity(MainActivity::class.java).get()

        assertEquals(
            MobileNotificationBridge.CAPABILITY,
            MobileNotificationBridge(activity).capability(),
        )
    }

    @Test
    fun payloadPreservesSenderBodyAndRuaUri() {
        val request = parseUserNotificationPayload(
            """
            {
              "target_device_id": "device-a",
              "sender": {
                "id": "agent:researcher",
                "name": "Research Agent"
              },
              "title": "Run complete",
              "body": "The requested analysis is ready.",
              "rua_uri": "rua://agents/researcher/chats/chat-42?turn=7#result"
            }
            """.trimIndent()
        )

        requireNotNull(request)
        assertEquals("device-a", request.targetDeviceId)
        assertEquals("agent:researcher", request.senderId)
        assertEquals("Research Agent", request.senderName)
        assertEquals("Run complete", request.title)
        assertEquals("The requested analysis is ready.", request.body)
        assertEquals(
            "rua://agents/researcher/chats/chat-42?turn=7#result",
            request.ruaUri,
        )
    }

    @Test
    fun malformedOrUnsafeRuaUriIsSilentlyDropped() {
        for (uri in listOf("", " https://rua.example", "rua:/missing-host", "https://rua.example")) {
            assertNull(safeRuaUriOrNull(uri))
        }
        assertEquals(
            "rua://agents/default/chats/chat-a",
            safeRuaUriOrNull("rua://agents/default/chats/chat-a"),
        )
    }

    @Test
    fun clickIntentIsExplicitAndCarriesOnlyTheValidatedRuaUri() {
        val context = RuntimeEnvironment.getApplication()
        val request = MobileUserNotification(
            targetDeviceId = "device-a",
            senderId = "agent:researcher",
            senderName = "Research Agent",
            title = "Run complete",
            body = null,
            ruaUri = "rua://agents/researcher/chats/chat-42",
        )

        val intent = userNotificationIntent(context, request)

        assertEquals(Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP, intent.flags)
        assertEquals(MainActivity::class.java.name, intent.component?.className)
        assertEquals(
            "rua://agents/researcher/chats/chat-42",
            intent.getStringExtra(MobileNotificationBridge.EXTRA_RUA_URI),
        )
        assertEquals(
            MobileNotificationBridge.ACTION_OPEN_USER_NOTIFICATION,
            intent.action,
        )
        assertFalse(MobileNotificationBridge.CHANNEL_ID.contains("bootstrap"))
        assertTrue(MobileNotificationBridge.CHANNEL_ID.contains("user_notifications"))
    }

    private fun notification(title: String) = MobileUserNotification(
        targetDeviceId = "device-a",
        senderId = "agent-a",
        senderName = "Agent A",
        title = title,
        body = "Done",
        ruaUri = "rua://agents/agent-a/chats/chat-1",
    )
}
