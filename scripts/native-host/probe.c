/* Bounded visual/input/lifecycle probe. Run only in an isolated Wine prefix. */
#define COBJMACROS
#include <windows.h>
#include <windowsx.h>
#include <d3d9.h>
#include <stdio.h>
#include <string.h>
static BOOL quit;
static unsigned close_requests, minimize_requests;
static BOOL reset_exclusive, reset_windowed;
static unsigned invalid_reset;

static LRESULT CALLBACK window_proc(HWND window, UINT message, WPARAM wp, LPARAM lp)
{
    if (message == WM_SYSCOMMAND && (wp & 0xfff0) == SC_MINIMIZE) {
        printf("minimize %u\n",++minimize_requests);
        if (minimize_requests==1) return 0;
    }
    if (message == WM_CLOSE) {
        printf("close %u\n", ++close_requests);
        if (close_requests >= 2) quit = TRUE;
        return 0; /* First request is deliberately cancelled. */
    }
    if (message == WM_KEYDOWN) {
        printf("key %u\n", (unsigned)wp);
        if (wp == 'M') { ShowWindow(window, SW_MINIMIZE); SetTimer(window, 1, 1500, NULL); }
        if (wp == 'E') reset_exclusive=TRUE;
        if (wp == 'W') reset_windowed=TRUE;
        if (wp == 'I') invalid_reset=1;
        if (wp == 'D') invalid_reset=2;
    }
    if (message == WM_TIMER) { KillTimer(window,1); ShowWindow(window, SW_RESTORE); }
    if (message == WM_SIZE) printf("size %u iconic %d\n", (unsigned)wp, IsIconic(window));
    if (message == WM_LBUTTONDOWN) {
        RECT client; POINT cursor;
        GetClientRect(window, &client); GetCursorPos(&cursor); ScreenToClient(window, &cursor);
        printf("click %d %d cursor %ld %ld client %ld %ld\n", GET_X_LPARAM(lp), GET_Y_LPARAM(lp), cursor.x, cursor.y, client.right, client.bottom);
    }
    return DefWindowProcA(window, message, wp, lp);
}
int main(int argc, char **argv)
{
    setvbuf(stdout, NULL, _IONBF, 0);
    WNDCLASSA wc = {0}; wc.lpfnWndProc = window_proc; wc.hInstance = GetModuleHandleA(NULL); wc.lpszClassName = "NativeHostProbe";
    if (!RegisterClassA(&wc)) return 1;
    HWND window = CreateWindowA(wc.lpszClassName, "Native host probe", WS_OVERLAPPEDWINDOW | WS_VISIBLE, 200, 200, 1280, 720, NULL, NULL, wc.hInstance, NULL);
    if (!window) return 2;
    IDirect3D9 *d3d = Direct3DCreate9(D3D_SDK_VERSION); if (!d3d) return 3;
    D3DPRESENT_PARAMETERS pp = {0}; pp.BackBufferWidth=1280; pp.BackBufferHeight=720; pp.BackBufferFormat=D3DFMT_X8R8G8B8;
    pp.BackBufferCount=1; pp.SwapEffect=D3DSWAPEFFECT_DISCARD; pp.hDeviceWindow=window; pp.Windowed=TRUE; pp.PresentationInterval=D3DPRESENT_INTERVAL_IMMEDIATE;
    IDirect3DDevice9 *device = NULL;
    HRESULT hr = IDirect3D9_CreateDevice(d3d,0,D3DDEVTYPE_HAL,window,D3DCREATE_SOFTWARE_VERTEXPROCESSING,&pp,&device);
    printf("create %08lx\n", (unsigned long)hr); if (FAILED(hr)) return 4;
    if (argc == 2) {
        HWND second=CreateWindowA(wc.lpszClassName,"Second probe",WS_OVERLAPPEDWINDOW | WS_VISIBLE,500,200,800,600,NULL,NULL,wc.hInstance,NULL);
        if (!second) return 10;
        IDirect3DDevice9 *other=NULL;
        D3DPRESENT_PARAMETERS next=pp; next.hDeviceWindow=second;
        hr=IDirect3D9_CreateDevice(d3d,0,D3DDEVTYPE_HAL,second,D3DCREATE_SOFTWARE_VERTEXPROCESSING,&next,&other);
        if (FAILED(hr)) return 11;
        printf("lifecycle both alive\n"); Sleep(1500);
        if (!strcmp(argv[1],"second-first")) {
            IDirect3DDevice9_Release(other);other=NULL;
            printf("lifecycle second released\n");Sleep(1500);
        }
        if (!strcmp(argv[1],"window-first")) {
            DestroyWindow(window);
            printf("lifecycle window destroyed\n");Sleep(1500);
        }
        IDirect3DDevice9_Release(device);device=NULL;
        printf("lifecycle owner released\n");Sleep(1500);
        if (other) IDirect3DDevice9_Release(other);
        DestroyWindow(second);
        if (IsWindow(window)) {
            hr=IDirect3D9_CreateDevice(d3d,0,D3DDEVTYPE_HAL,window,D3DCREATE_SOFTWARE_VERTEXPROCESSING,&pp,&device);
            if (FAILED(hr)) return 12;
            printf("lifecycle recreated\n");Sleep(1500);
            IDirect3DDevice9_Release(device);DestroyWindow(window);
        }
        IDirect3D9_Release(d3d);printf("teardown complete\n");return 0;
    }
    DWORD started=GetTickCount(), last=started; unsigned frames=0;
    while (!quit && GetTickCount()-started < 100000) {
        MSG message; unsigned count=0;
        while (PeekMessageA(&message,NULL,0,0,PM_REMOVE)) {
            TranslateMessage(&message); DispatchMessageA(&message);
            if (++count > 4096) { printf("FAIL event flood\n"); return 5; }
        }
        if (reset_exclusive || reset_windowed || invalid_reset) {
            D3DPRESENT_PARAMETERS next=pp;
            next.Windowed=reset_windowed;
            next.BackBufferWidth=1280; next.BackBufferHeight=720;
            if (invalid_reset==1) next.MultiSampleType=17;
            if (invalid_reset==2) { next.EnableAutoDepthStencil=TRUE; next.AutoDepthStencilFormat=(D3DFORMAT)0x7fffffff; }
            hr=IDirect3DDevice9_Reset(device,&next);
            printf("reset windowed %d invalid %d result %08lx\n",next.Windowed,invalid_reset,(unsigned long)hr);
            if ((invalid_reset && SUCCEEDED(hr)) || (!invalid_reset && FAILED(hr))) return 8;
            if (SUCCEEDED(hr)) pp=next;
            reset_exclusive=reset_windowed=invalid_reset=FALSE;
        }
        LONG w=(LONG)pp.BackBufferWidth,h=(LONG)pp.BackBufferHeight;
        D3DRECT rects[4]={{0,0,w/2,h/2},{w/2,0,w,h/2},{0,h/2,w/2,h},{w/2,h/2,w,h}};
        DWORD colors[]={0xffff0000,0xff00ff00,0xff0000ff,0xffffff00};
        for (unsigned i=0;i<4;i++) IDirect3DDevice9_Clear(device,1,&rects[i],D3DCLEAR_TARGET,colors[i],1,0);
        hr=IDirect3DDevice9_Present(device,NULL,NULL,NULL,NULL);
        if (FAILED(hr)) { printf("FAIL present %08lx\n", (unsigned long)hr); return 6; }
        frames++;
        if (GetTickCount()-last >= 1000) {
            RECT client; GetClientRect(window,&client);
            IDirect3DSurface9 *bb=NULL; D3DSURFACE_DESC desc={0};
            IDirect3DDevice9_GetBackBuffer(device,0,0,D3DBACKBUFFER_TYPE_MONO,&bb);
            IDirect3DSurface9_GetDesc(bb,&desc); IDirect3DSurface9_Release(bb);
            printf("frame %u client %ld %ld backbuffer %u %u foreground %d\n",frames,client.right,client.bottom,desc.Width,desc.Height,GetForegroundWindow()==window);
            if (desc.Width!=pp.BackBufferWidth || desc.Height!=pp.BackBufferHeight) {printf("FAIL backbuffer changed\n");return 7;}
            frames=0;last=GetTickCount();
        }
        Sleep(1);
    }
    printf("teardown begin\n");
    IDirect3DDevice9_Release(device); IDirect3D9_Release(d3d);
    DestroyWindow(window); printf("teardown complete\n");
    return 0;
}
