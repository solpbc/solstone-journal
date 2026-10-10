// Compile/link probe only; target execution is outside this build evidence.
#include "fpdfview.h"
#include "fpdf_doc.h"
#include "fpdf_text.h"
#include "fpdf_edit.h"

static void (*volatile required_symbols[])(void) = {
    (void (*)(void))FPDF_InitLibrary,
    (void (*)(void))FPDF_DestroyLibrary,
    (void (*)(void))FPDF_LoadDocument,
    (void (*)(void))FPDF_GetLastError,
    (void (*)(void))FPDF_CloseDocument,
    (void (*)(void))FPDF_GetPageCount,
    (void (*)(void))FPDF_GetMetaText,
    (void (*)(void))FPDF_GetSecurityHandlerRevision,
    (void (*)(void))FPDF_LoadPage,
    (void (*)(void))FPDF_ClosePage,
    (void (*)(void))FPDF_GetPageWidthF,
    (void (*)(void))FPDF_GetPageHeightF,
    (void (*)(void))FPDFText_LoadPage,
    (void (*)(void))FPDFText_ClosePage,
    (void (*)(void))FPDFText_CountChars,
    (void (*)(void))FPDFText_GetText,
    (void (*)(void))FPDFPage_CountObjects,
    (void (*)(void))FPDFPage_GetObject,
    (void (*)(void))FPDFPageObj_GetType,
    (void (*)(void))FPDFPageObj_GetBounds,
    (void (*)(void))FPDFBitmap_CreateEx,
    (void (*)(void))FPDFBitmap_FillRect,
    (void (*)(void))FPDF_RenderPageBitmap,
    (void (*)(void))FPDFBitmap_GetBuffer,
    (void (*)(void))FPDFBitmap_GetStride,
    (void (*)(void))FPDFBitmap_Destroy,
};
int main(int argc, char **argv) {
    (void)argv;
    unsigned int count = sizeof(required_symbols) / sizeof(required_symbols[0]);
    return required_symbols[(unsigned int)argc % count] ? 0 : 1;
}
