package com.ausha.receiver

import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Build
import android.util.Log
import kotlinx.coroutines.channels.awaitClose
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.callbackFlow

data class Sender(val name: String, val host: String, val port: Int)

/**
 * Finds senders advertising `_ausha._tcp` on the local network.
 *
 * mDNS is unreliable across VLANs, on enterprise WiFi with client isolation,
 * and on some consumer routers, so the UI always keeps a manual entry path.
 */
class Discovery(private val context: Context) {

    fun senders(): Flow<List<Sender>> = callbackFlow {
        val nsd = context.getSystemService(Context.NSD_SERVICE) as NsdManager
        val wifi = context.applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        // Without this the radio filters the multicast that mDNS rides on.
        val multicastLock = wifi.createMulticastLock("ausha:mdns").apply {
            setReferenceCounted(false)
            acquire()
        }

        val found = LinkedHashMap<String, Sender>()

        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {}

            override fun onServiceFound(info: NsdServiceInfo) {
                resolve(nsd, info) { sender ->
                    found[sender.name] = sender
                    trySend(found.values.toList())
                }
            }

            override fun onServiceLost(info: NsdServiceInfo) {
                found.remove(info.serviceName)
                trySend(found.values.toList())
            }

            override fun onDiscoveryStopped(serviceType: String) {}

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                Log.w(TAG, "discovery failed to start: $errorCode")
                trySend(emptyList())
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}
        }

        nsd.discoverServices(SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, listener)
        awaitClose {
            runCatching { nsd.stopServiceDiscovery(listener) }
            runCatching { multicastLock.release() }
        }
    }

    private fun resolve(nsd: NsdManager, info: NsdServiceInfo, onResolved: (Sender) -> Unit) {
        val callback = object : NsdManager.ResolveListener {
            override fun onResolveFailed(info: NsdServiceInfo, errorCode: Int) {
                Log.w(TAG, "resolve failed for ${info.serviceName}: $errorCode")
            }

            override fun onServiceResolved(resolved: NsdServiceInfo) {
                val host = if (Build.VERSION.SDK_INT >= 34) {
                    resolved.hostAddresses.firstOrNull()?.hostAddress
                } else {
                    @Suppress("DEPRECATION")
                    resolved.host?.hostAddress
                } ?: return
                onResolved(Sender(resolved.serviceName, host, resolved.port))
            }
        }
        @Suppress("DEPRECATION")
        nsd.resolveService(info, callback)
    }

    companion object {
        private const val TAG = "ausha"
        const val SERVICE_TYPE = "_ausha._tcp."
    }
}
