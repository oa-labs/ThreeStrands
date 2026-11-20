package app.threestrands.mail

import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import org.junit.runner.RunWith
import java.util.UUID

@RunWith(AndroidJUnit4::class)
class SyncSecretStoreTest {
    @Test
    fun aNewStoreReadsSavedValuesAndAnEntryCanBeDeleted() {
        val context = InstrumentationRegistry.getInstrumentation().targetContext
        val service = "test." + UUID.randomUUID()
        val first = SyncSecretStore(context)
        val second = SyncSecretStore(context)
        try {
            assertNull(first.read(service, "device-key"))
            first.write(service, "device-key", "fixture device key")
            first.write(service, "connector", "fixture connector credentials")
            assertEquals("fixture device key", second.read(service, "device-key"))
            assertEquals("fixture connector credentials", second.read(service, "connector"))
            second.delete(service, "device-key")
            assertNull(first.read(service, "device-key"))
            assertEquals("fixture connector credentials", first.read(service, "connector"))
        } finally {
            first.delete(service, "device-key")
            first.delete(service, "connector")
        }
    }
}
