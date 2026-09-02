package com.ausha.receiver

import android.Manifest
import android.content.Intent
import android.os.Build
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.*
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.*
import androidx.compose.runtime.*
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import com.google.accompanist.permissions.ExperimentalPermissionsApi
import com.google.accompanist.permissions.isGranted
import com.google.accompanist.permissions.rememberPermissionState
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow

class MainActivity : ComponentActivity() {

    private val pairingFromLink = MutableStateFlow<Pairing?>(null)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        consumeLink(intent)
        setContent { MaterialTheme(colorScheme = darkColorScheme()) { AushaApp(pairingFromLink) } }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        consumeLink(intent)
    }

    private fun consumeLink(intent: Intent?) {
        intent?.data?.toString()?.let { Pairing.parse(it) }?.let { pairingFromLink.value = it }
    }
}

@OptIn(ExperimentalPermissionsApi::class, ExperimentalMaterial3Api::class)
@Composable
fun AushaApp(links: MutableStateFlow<Pairing?>) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val discovery = remember { Discovery(context) }
    val senders by discovery.senders().collectAsState(emptyList())
    val link by links.collectAsState()

    var host by rememberSaveable { mutableStateOf("") }
    var port by rememberSaveable { mutableStateOf(Pairing.DEFAULT_PORT.toString()) }
    var token by rememberSaveable { mutableStateOf("") }
    var scanning by rememberSaveable { mutableStateOf(false) }
    var latency by rememberSaveable { mutableStateOf(AudioEngine.Latency.Balanced) }
    var stats by remember { mutableStateOf(Stats()) }
    var state by remember { mutableStateOf(Playback.state) }

    val notifications = if (Build.VERSION.SDK_INT >= 33) {
        rememberPermissionState(Manifest.permission.POST_NOTIFICATIONS)
    } else null
    val camera = rememberPermissionState(Manifest.permission.CAMERA)

    // A pairing link carries everything needed, so acting on it immediately is
    // the point: scanning a code should start playback, not fill in a form.
    LaunchedEffect(link) {
        link?.let {
            host = it.host
            port = it.port.toString()
            token = it.token
            scanning = false
            links.value = null
            PlaybackService.start(context, it.host, it.port, it.token, Build.MODEL, latency)
        }
    }

    LaunchedEffect(Unit) {
        notifications?.takeIf { !it.status.isGranted }?.launchPermissionRequest()
        while (true) {
            stats = Playback.engine.stats
            state = Playback.state
            delay(500)
        }
    }

    if (scanning) {
        Scaffold { padding ->
            Column(Modifier.padding(padding).fillMaxSize()) {
                if (camera.status.isGranted) {
                    QrScanner(Modifier.weight(1f)) { links.value = it }
                } else {
                    LaunchedEffect(Unit) { camera.launchPermissionRequest() }
                    Box(Modifier.weight(1f), Alignment.Center) {
                        Text("Camera permission is needed to scan the pairing code")
                    }
                }
                TextButton(onClick = { scanning = false }, modifier = Modifier.padding(16.dp)) {
                    Text("Cancel")
                }
            }
        }
        return
    }

    Scaffold(
        topBar = {
            TopAppBar(
                title = { Text("Ausha") },
                navigationIcon = {
                    Icon(
                        painterResource(R.drawable.ic_ausha_mark),
                        contentDescription = null,
                        modifier = Modifier.padding(horizontal = 16.dp),
                    )
                },
            )
        },
    ) { padding ->
        Column(
            Modifier.padding(padding).padding(16.dp).fillMaxSize().verticalScroll(rememberScrollState()),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            StatusCard(state, stats, Playback.engine.failure)

            if (senders.isNotEmpty()) {
                Text("Senders on this network", style = MaterialTheme.typography.titleSmall)
                LazyColumn(Modifier.heightIn(max = 160.dp)) {
                    items(senders) { sender ->
                        ListItem(
                            headlineContent = { Text(sender.name) },
                            supportingContent = { Text("${sender.host}:${sender.port}") },
                            modifier = Modifier.clickable {
                                host = sender.host
                                port = sender.port.toString()
                            },
                        )
                    }
                }
            }

            OutlinedTextField(
                value = host,
                onValueChange = { host = it },
                label = { Text("Sender address") },
                singleLine = true,
                modifier = Modifier.fillMaxWidth(),
            )
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                OutlinedTextField(
                    value = port,
                    onValueChange = { port = it.filter(Char::isDigit).take(5) },
                    label = { Text("Port") },
                    singleLine = true,
                    modifier = Modifier.width(120.dp),
                )
                OutlinedTextField(
                    value = token,
                    onValueChange = { token = it },
                    label = { Text("Pairing token") },
                    singleLine = true,
                    modifier = Modifier.weight(1f),
                )
            }

            Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
                Text("Latency", style = MaterialTheme.typography.titleSmall)
                SingleChoiceSegmentedButtonRow(Modifier.fillMaxWidth()) {
                    AudioEngine.Latency.entries.forEachIndexed { index, option ->
                        SegmentedButton(
                            selected = latency == option,
                            onClick = { latency = option },
                            shape = SegmentedButtonDefaults.itemShape(
                                index,
                                AudioEngine.Latency.entries.size,
                            ),
                        ) { Text(option.name) }
                    }
                }
                Text(
                    when (latency) {
                        AudioEngine.Latency.Low -> "Least delay. Best on a quiet network."
                        AudioEngine.Latency.Balanced -> "Default. Absorbs ordinary WiFi loss."
                        AudioEngine.Latency.Stable -> "Deepest buffer. For a weak signal."
                    },
                    style = MaterialTheme.typography.bodySmall,
                )
            }

            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                Button(
                    onClick = {
                        PlaybackService.start(
                            context,
                            host.trim(),
                            port.toIntOrNull() ?: Pairing.DEFAULT_PORT,
                            token.trim(),
                            Build.MODEL,
                            latency,
                        )
                    },
                    enabled = host.isNotBlank() && token.isNotBlank() &&
                        state != AudioEngine.State.Playing,
                ) { Text("Connect") }

                OutlinedButton(
                    onClick = { PlaybackService.stop(context) },
                    enabled = state == AudioEngine.State.Playing ||
                        state == AudioEngine.State.Connecting,
                ) { Text("Stop") }

                OutlinedButton(onClick = { scanning = true }) { Text("Scan QR") }
            }
        }
    }
}

@Composable
private fun StatusCard(state: AudioEngine.State, stats: Stats, failure: String?) {
    Card(Modifier.fillMaxWidth()) {
        Column(Modifier.padding(16.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
            Text(
                when (state) {
                    AudioEngine.State.Idle -> "Not connected"
                    AudioEngine.State.Connecting -> "Connecting…"
                    AudioEngine.State.Playing -> "Playing"
                    AudioEngine.State.Stopped -> "Stopped"
                    AudioEngine.State.Failed -> "Failed: ${failure ?: "unknown"}"
                },
                style = MaterialTheme.typography.titleMedium,
                fontWeight = FontWeight.SemiBold,
            )
            if (state == AudioEngine.State.Playing || stats.received > 0) {
                Metric("Latency", "%.0f ms".format(stats.latencyMs))
                Metric("Buffer", "${stats.depthMs} / ${stats.targetMs} ms")
                Metric("Jitter", "%.1f ms".format(stats.jitterMs))
                Metric("Loss", "%.2f%%".format(stats.lossPercent))
                Metric("Recovered by FEC", stats.recovered.toString())
                Metric("Concealed", stats.concealed.toString())
                Metric("Underruns", stats.underruns.toString())
                Metric("Clock correction", "%.4f".format(stats.ratio))
            }
        }
    }
}

@Composable
private fun Metric(label: String, value: String) {
    Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.SpaceBetween) {
        Text(label, style = MaterialTheme.typography.bodyMedium)
        Text(value, style = MaterialTheme.typography.bodyMedium, fontFamily = FontFamily.Monospace)
    }
}
