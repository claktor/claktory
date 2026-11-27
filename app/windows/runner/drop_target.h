#ifndef RUNNER_DROP_TARGET_H_
#define RUNNER_DROP_TARGET_H_

#include <windows.h>
#include <ole2.h>
#include <shobjidl.h>

#include <functional>
#include <string>
#include <vector>

// Files dragged onto the window from Explorer (lib/attachments.dart's
// FileDrop), as the Mac's MainFlutterWindow.swift does: [over] says how many
// are over the window (0 when they leave or are dropped), and [dropped] hands
// over their paths, only while [takes_files] says a message box takes them.
// Explorer shows its usual drag image over the window (IDropTargetHelper).
//
// Registered on the Flutter view's own window, which covers the whole client
// area: RegisterDragDrop needs OLE, so main.cpp starts it with OleInitialize.
class DropTarget : public IDropTarget {
 public:
  DropTarget(HWND window, std::function<void(int count)> over,
             std::function<void(std::vector<std::string> paths)> dropped);

  // Registers it with [window_]; false (logged) when the window already has
  // one or OLE refuses.
  bool Register();
  void Revoke();

  void set_takes_files(bool takes) { takes_files_ = takes; }

  // IUnknown
  HRESULT STDMETHODCALLTYPE QueryInterface(REFIID riid, void** object) override;
  ULONG STDMETHODCALLTYPE AddRef() override;
  ULONG STDMETHODCALLTYPE Release() override;

  // IDropTarget
  HRESULT STDMETHODCALLTYPE DragEnter(IDataObject* data, DWORD keys,
                                      POINTL point, DWORD* effect) override;
  HRESULT STDMETHODCALLTYPE DragOver(DWORD keys, POINTL point,
                                     DWORD* effect) override;
  HRESULT STDMETHODCALLTYPE DragLeave() override;
  HRESULT STDMETHODCALLTYPE Drop(IDataObject* data, DWORD keys, POINTL point,
                                 DWORD* effect) override;

 private:
  virtual ~DropTarget();

  // The paths of the files [data] holds (CF_HDROP); none for anything else.
  static std::vector<std::string> FilesIn(IDataObject* data);

  DWORD Effect() const;

  LONG references_ = 1;
  HWND window_;
  std::function<void(int)> over_;
  std::function<void(std::vector<std::string>)> dropped_;
  IDropTargetHelper* helper_ = nullptr;
  bool registered_ = false;
  bool takes_files_ = false;
  // How many files the drag over the window carries; 0 when it carries none.
  int count_ = 0;
};

#endif  // RUNNER_DROP_TARGET_H_
