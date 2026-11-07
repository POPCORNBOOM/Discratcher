// MMC pass-through on macOS. Thin on purpose: open a device, run one CDB, report status
// and timestamps. Everything about what to send lives in Rust.
//
// Timestamps are taken here, right around ExecuteTaskSync, so they cover the kernel and
// the bus but not any scheduling on the Rust side.

#include <CoreFoundation/CoreFoundation.h>
#include <IOKit/IOBSD.h>
#include <IOKit/IOCFPlugIn.h>
#include <IOKit/IOKitLib.h>
#include <IOKit/scsi/SCSICmds_INQUIRY_Definitions.h>
#include <IOKit/scsi/SCSITaskLib.h>
#include <IOKit/storage/IOStorageDeviceCharacteristics.h>
#include <mach/mach_time.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef struct {
    IOCFPlugInInterface **plugin;
    MMCDeviceInterface **mmc;
    SCSITaskDeviceInterface **dev;
    SCSITaskInterface **task;
    mach_timebase_info_data_t base;
    int exclusive;
} ds_handle;

// Walk up from an IOMedia with this BSD name to the service owning the drive.
static io_service_t find_drive(const char *bsd) {
    CFMutableDictionaryRef match = IOBSDNameMatching(kIOMainPortDefault, 0, bsd);
    if (!match) return 0;
    io_service_t cur = IOServiceGetMatchingService(kIOMainPortDefault, match);
    while (cur) {
        CFTypeRef uuid = IORegistryEntryCreateCFProperty(
            cur, CFSTR(kIOPropertySCSITaskDeviceCategory), kCFAllocatorDefault, 0);
        if (uuid) {
            CFRelease(uuid);
            return cur;
        }
        io_service_t parent = 0;
        kern_return_t kr = IORegistryEntryGetParentEntry(cur, kIOServicePlane, &parent);
        IOObjectRelease(cur);
        if (kr != KERN_SUCCESS) return 0;
        cur = parent;
    }
    return 0;
}

// Drives with no disc have no IOMedia, so also search the services directly.
static io_service_t first_drive(void) {
    static const char *classes[] = {"IODVDServices", "IOCompactDiscServices", "IOBDServices"};
    for (size_t i = 0; i < sizeof classes / sizeof *classes; i++) {
        io_service_t s = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching(classes[i]));
        if (s) return s;
    }
    return 0;
}

static int open_plugin(ds_handle *h, io_service_t svc) {
    SInt32 score = 0;
    IOReturn kr = IOCreatePlugInInterfaceForService(
        svc, kIOMMCDeviceUserClientTypeID, kIOCFPlugInInterfaceID, &h->plugin, &score);
    if (kr == kIOReturnSuccess && h->plugin) {
        HRESULT hr = (*h->plugin)->QueryInterface(
            h->plugin, CFUUIDGetUUIDBytes(kIOMMCDeviceInterfaceID), (LPVOID *)&h->mmc);
        if (hr == S_OK && h->mmc) {
            h->dev = (*h->mmc)->GetSCSITaskDeviceInterface(h->mmc);
            if (h->dev) return 0;
        }
    }
    if (h->plugin) {
        IODestroyPlugInInterface(h->plugin);
        h->plugin = NULL;
    }
    h->mmc = NULL;
    kr = IOCreatePlugInInterfaceForService(
        svc, kIOSCSITaskDeviceUserClientTypeID, kIOCFPlugInInterfaceID, &h->plugin, &score);
    if (kr != kIOReturnSuccess || !h->plugin) return kr ? kr : -1;
    HRESULT hr = (*h->plugin)->QueryInterface(
        h->plugin, CFUUIDGetUUIDBytes(kIOSCSITaskDeviceInterfaceID), (LPVOID *)&h->dev);
    return (hr == S_OK && h->dev) ? 0 : -2;
}

void ds_close(ds_handle *h) {
    if (!h) return;
    if (h->task) (*h->task)->Release(h->task);
    if (h->dev) {
        if (h->exclusive) (*h->dev)->ReleaseExclusiveAccess(h->dev);
        if (!h->mmc) (*h->dev)->Release(h->dev);
    }
    if (h->mmc) (*h->mmc)->Release(h->mmc);
    if (h->plugin) IODestroyPlugInInterface(h->plugin);
    free(h);
}

// `bsd` may be NULL for the first drive. Returns NULL and an IOReturn in `*err`.
ds_handle *ds_open(const char *bsd, int *err) {
    io_service_t svc = bsd ? find_drive(bsd) : 0;
    if (!svc) svc = first_drive();
    if (!svc) {
        *err = -3;
        return NULL;
    }
    ds_handle *h = calloc(1, sizeof *h);
    mach_timebase_info(&h->base);
    int rc = open_plugin(h, svc);
    IOObjectRelease(svc);
    if (rc) {
        *err = rc;
        ds_close(h);
        return NULL;
    }
    IOReturn kr = (*h->dev)->ObtainExclusiveAccess(h->dev);
    if (kr != kIOReturnSuccess) {
        *err = kr;
        ds_close(h);
        return NULL;
    }
    h->exclusive = 1;
    h->task = (*h->dev)->CreateSCSITask(h->dev);
    if (!h->task) {
        *err = -4;
        ds_close(h);
        return NULL;
    }
    *err = 0;
    return h;
}

// direction: 0 none, 2 read. Writes are not offered: nothing here should ever touch media.
int ds_exec(ds_handle *h, const uint8_t *cdb, uint8_t cdb_len, int direction, uint8_t *buf,
            uint32_t len, uint32_t timeout_ms, uint8_t *status, uint8_t sense[3],
            double *t_end_ns) {
    if (direction != 0 && direction != 2) return -5;
    SCSITaskInterface **t = h->task;
    IOVirtualRange range = {(IOVirtualAddress)buf, len};
    SCSICommandDescriptorBlock block;
    memset(block, 0, sizeof block);
    memcpy(block, cdb, cdb_len > sizeof block ? sizeof block : cdb_len);
    (*t)->ResetForNewTask(t);
    IOReturn kr = (*t)->SetCommandDescriptorBlock(t, block, cdb_len);
    if (kr) return kr;
    kr = (*t)->SetScatterGatherEntries(
        t, direction ? &range : NULL, direction ? 1 : 0, direction ? len : 0,
        direction ? kSCSIDataTransfer_FromTargetToInitiator : kSCSIDataTransfer_NoDataTransfer);
    if (kr) return kr;
    kr = (*t)->SetTimeoutDuration(t, timeout_ms);
    if (kr) return kr;
    SCSI_Sense_Data sd;
    memset(&sd, 0, sizeof sd);
    SCSITaskStatus st = kSCSITaskStatus_No_Status;
    UInt64 done = 0;
    kr = (*t)->ExecuteTaskSync(t, &sd, &st, &done);
    uint64_t now = mach_absolute_time();
    *t_end_ns = (double)now * h->base.numer / h->base.denom;
    *status = (uint8_t)st;
    sense[0] = sd.SENSE_KEY & 0x0F;
    sense[1] = sd.ADDITIONAL_SENSE_CODE;
    sense[2] = sd.ADDITIONAL_SENSE_CODE_QUALIFIER;
    return kr;
}
