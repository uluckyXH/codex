#include <database/pasteboard/oh_pasteboard.h>
#include <database/udmf/udmf.h>
#include <database/udmf/uds.h>
/* Compile/link only; never executed. Each typed pointer checks a Rust FFI signature. */
OH_Pasteboard *(*volatile p1)(void) = OH_Pasteboard_Create;
void (*volatile p2)(OH_Pasteboard *) = OH_Pasteboard_Destroy;
int (*volatile p3)(OH_Pasteboard *, OH_UdmfData *) = OH_Pasteboard_SetData;
OH_UdmfData *(*volatile p4)(OH_Pasteboard *, int *) = OH_Pasteboard_GetData;
OH_UdmfData *(*volatile p5)(void) = OH_UdmfData_Create;
void (*volatile p6)(OH_UdmfData *) = OH_UdmfData_Destroy;
int (*volatile p7)(OH_UdmfData *, OH_UdmfRecord *) = OH_UdmfData_AddRecord;
int (*volatile p8)(OH_UdmfData *, OH_UdsPlainText *) = OH_UdmfData_GetPrimaryPlainText;
OH_UdmfRecord *(*volatile p9)(void) = OH_UdmfRecord_Create;
void (*volatile p10)(OH_UdmfRecord *) = OH_UdmfRecord_Destroy;
int (*volatile p11)(OH_UdmfRecord *, OH_UdsPlainText *) = OH_UdmfRecord_AddPlainText;
OH_UdsPlainText *(*volatile p12)(void) = OH_UdsPlainText_Create;
void (*volatile p13)(OH_UdsPlainText *) = OH_UdsPlainText_Destroy;
int (*volatile p14)(OH_UdsPlainText *, const char *) = OH_UdsPlainText_SetContent;
const char *(*volatile p15)(OH_UdsPlainText *) = OH_UdsPlainText_GetContent;
int main(void) { return !p1 || !p2 || !p3 || !p4 || !p5 || !p6 || !p7 || !p8 || !p9 || !p10 || !p11 || !p12 || !p13 || !p14 || !p15; }
