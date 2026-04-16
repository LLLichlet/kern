#include "llvm/ADT/DenseMap.h"
#include "llvm/ADT/SmallString.h"
#include "llvm/ADT/SmallVector.h"
#include "llvm/ADT/STLExtras.h"
#include "llvm/ADT/StringMap.h"
#include "llvm/LTO/Config.h"
#include "llvm/LTO/LTO.h"
#include "llvm/Support/Caching.h"
#include "llvm/Support/Error.h"
#include "llvm/Support/FileSystem.h"
#include "llvm/Support/MemoryBuffer.h"
#include "llvm/Support/Mutex.h"
#include "llvm/Support/Path.h"
#include "llvm/Support/Threading.h"
#include "llvm/Support/raw_ostream.h"

#include <cstddef>
#include <cstdint>
#include <cstring>
#include <memory>
#include <new>
#include <system_error>
#include <utility>

namespace {

using llvm::Expected;
using llvm::MemoryBuffer;
using llvm::MemoryBufferRef;
using llvm::SmallString;
using llvm::SmallVector;
using llvm::StringMap;
using llvm::StringRef;
using llvm::Twine;

struct BridgeOutput {
  enum class Kind {
    Buffer,
    File,
  };

  Kind KindTag = Kind::Buffer;
  SmallVector<char, 0> Buffer;
  SmallString<0> Path;
};

struct BridgeModule {
  SmallString<0> Identifier;
  SmallVector<char, 0> Bitcode;
};

class ThinLtoSession;

class BufferStream final : public llvm::raw_pwrite_stream {
public:
  BufferStream(ThinLtoSession &Session, unsigned Task)
      : Session(Session), Task(Task) {}

  ~BufferStream() override;

  std::uint64_t current_pos() const override { return Buffer.size(); }

private:
  void write_impl(const char *Ptr, size_t Size) override {
    Buffer.append(Ptr, Ptr + Size);
  }

  void pwrite_impl(const char *Ptr, size_t Size, std::uint64_t Offset) override {
    if (Offset + Size > Buffer.size()) {
      Buffer.resize_for_overwrite(static_cast<size_t>(Offset + Size));
    }
    std::memcpy(Buffer.data() + Offset, Ptr, Size);
  }

  ThinLtoSession &Session;
  unsigned Task;
  SmallVector<char, 0> Buffer;
};

class ThinLtoSession {
public:
  bool set_cpu(const char *Cpu) {
    if (Cpu == nullptr) {
      set_error("LLVM ThinLTO CPU string was null");
      return false;
    }
    CpuName = Cpu;
    return true;
  }

  bool set_generated_objects_dir(const char *Dir) {
    return set_path(Dir, GeneratedObjectsDir, HasGeneratedObjectsDir,
                    "generated objects");
  }

  bool set_cache_dir(const char *Dir) {
    return set_path(Dir, CacheDir, HasCacheDir, "cache");
  }

  bool add_module(const char *Identifier, const std::uint8_t *Bitcode, size_t Size) {
    if (Identifier == nullptr) {
      set_error("LLVM ThinLTO module identifier was null");
      return false;
    }
    if (Bitcode == nullptr && Size != 0) {
      set_error("LLVM ThinLTO module bitcode buffer was null");
      return false;
    }

    BridgeModule Module;
    Module.Identifier = Identifier;
    Module.Bitcode.append(reinterpret_cast<const char *>(Bitcode),
                          reinterpret_cast<const char *>(Bitcode) + Size);
    Modules.push_back(std::move(Module));
    return true;
  }

  bool process() {
    clear_outputs();

    llvm::lto::Config Config;
    Config.CPU = CpuName.c_str();

    auto Lto = llvm::lto::LTO(
        std::move(Config),
        llvm::lto::createInProcessThinBackend(
            llvm::heavyweight_hardware_concurrency()));

    StringMap<size_t> PrevailingGlobals;

    for (size_t ModuleIndex = 0; ModuleIndex < Modules.size(); ++ModuleIndex) {
      const auto &Module = Modules[ModuleIndex];
      auto Input = llvm::lto::InputFile::create(
          MemoryBufferRef(StringRef(Module.Bitcode.data(), Module.Bitcode.size()),
                          Module.Identifier.str()));
      if (!Input) {
        auto Message = llvm::toString(Input.takeError());
        set_error(Message);
        return false;
      }

      SmallVector<llvm::lto::SymbolResolution, 0> Resolutions;
      Resolutions.reserve((*Input)->symbols().size());
      for (const auto &Symbol : (*Input)->symbols()) {
        llvm::lto::SymbolResolution Resolution;
        const bool IsUndefined = Symbol.isUndefined();
        const bool IsLocal = !Symbol.isGlobal();

        Resolution.Prevailing = !IsUndefined;
        if (!IsUndefined && !IsLocal) {
          auto It = PrevailingGlobals.try_emplace(Symbol.getName(), ModuleIndex);
          Resolution.Prevailing = It.second || It.first->second == ModuleIndex;
        }

        Resolution.FinalDefinitionInLinkageUnit = !IsUndefined;
        Resolution.VisibleToRegularObj = !IsUndefined && !IsLocal;
        Resolution.ExportDynamic = false;
        Resolution.LinkerRedefined = false;
        Resolutions.push_back(Resolution);
      }

      if (auto Err = Lto.add(std::move(*Input), Resolutions)) {
        auto Message = llvm::toString(std::move(Err));
        set_error(Message);
        return false;
      }
    }

    auto AddStream = [this](size_t Task, const Twine &ModuleName)
        -> Expected<std::unique_ptr<llvm::CachedFileStream>> {
      (void)ModuleName;
      if (!HasGeneratedObjectsDir) {
        auto Stream = std::make_unique<BufferStream>(*this, static_cast<unsigned>(Task));
        return std::make_unique<llvm::CachedFileStream>(std::move(Stream));
      }

      SmallString<0> Path = make_output_path(static_cast<unsigned>(Task));
      std::error_code Ec;
      auto Stream =
          std::make_unique<llvm::raw_fd_ostream>(Path, Ec, llvm::sys::fs::OF_None);
      if (Ec) {
        return llvm::errorCodeToError(Ec);
      }
      record_file_output(static_cast<unsigned>(Task), Path);
      return std::make_unique<llvm::CachedFileStream>(std::move(Stream));
    };

    llvm::FileCache Cache;
    if (HasCacheDir) {
      auto AddBuffer = [this](unsigned Task, const Twine &ModuleName,
                              std::unique_ptr<MemoryBuffer> Buffer) {
        (void)ModuleName;
        if (HasGeneratedObjectsDir) {
          SmallString<0> Path = make_output_path(Task);
          std::error_code Ec;
          llvm::raw_fd_ostream Stream(Path, Ec, llvm::sys::fs::OF_None);
          if (Ec) {
            set_error(Ec.message());
            return;
          }
          Stream.write(Buffer->getBufferStart(), Buffer->getBufferSize());
          Stream.close();
          record_file_output(Task, Path);
          return;
        }

        SmallVector<char, 0> Bytes;
        Bytes.append(Buffer->getBufferStart(),
                     Buffer->getBufferStart() + Buffer->getBufferSize());
        record_buffer_output(Task, std::move(Bytes));
      };

      auto CacheOrErr = llvm::localCache("ThinLTO", "thinlto", CacheDir.c_str(), AddBuffer);
      if (!CacheOrErr) {
        auto Message = llvm::toString(CacheOrErr.takeError());
        set_error(Message);
        return false;
      }
      Cache = std::move(*CacheOrErr);
    }

    if (auto Err = Lto.run(AddStream, Cache)) {
      auto Message = llvm::toString(std::move(Err));
      set_error(Message);
      return false;
    }

    finalize_outputs();
    if (OrderedOutputs.empty()) {
      set_error("LLVM ThinLTO did not produce any object files");
      return false;
    }
    return true;
  }

  size_t object_count() const { return OrderedOutputs.size(); }

  bool object_is_file(size_t Index) const {
    return Index < OrderedOutputs.size() &&
           OrderedOutputs[Index].KindTag == BridgeOutput::Kind::File;
  }

  size_t object_path_len(size_t Index) const {
    if (Index >= OrderedOutputs.size() ||
        OrderedOutputs[Index].KindTag != BridgeOutput::Kind::File) {
      return 0;
    }
    return OrderedOutputs[Index].Path.size();
  }

  bool copy_object_path(size_t Index, char *Dest, size_t DestLen) const {
    if (Index >= OrderedOutputs.size() || Dest == nullptr) {
      return false;
    }
    const auto &Output = OrderedOutputs[Index];
    if (Output.KindTag != BridgeOutput::Kind::File || DestLen < Output.Path.size()) {
      return false;
    }
    std::memcpy(Dest, Output.Path.data(), Output.Path.size());
    return true;
  }

  size_t object_buffer_len(size_t Index) const {
    if (Index >= OrderedOutputs.size() ||
        OrderedOutputs[Index].KindTag != BridgeOutput::Kind::Buffer) {
      return 0;
    }
    return OrderedOutputs[Index].Buffer.size();
  }

  bool copy_object_buffer(size_t Index, std::uint8_t *Dest, size_t DestLen) const {
    if (Index >= OrderedOutputs.size() || Dest == nullptr) {
      return false;
    }
    const auto &Output = OrderedOutputs[Index];
    if (Output.KindTag != BridgeOutput::Kind::Buffer ||
        DestLen < Output.Buffer.size()) {
      return false;
    }
    std::memcpy(Dest, Output.Buffer.data(), Output.Buffer.size());
    return true;
  }

  const char *last_error() const {
    return const_cast<SmallString<0> &>(LastError).c_str();
  }

  void record_buffer_output(unsigned Task, SmallVector<char, 0> Buffer) {
    llvm::sys::SmartScopedLock<true> Guard(Mutex);
    BridgeOutput Output;
    Output.KindTag = BridgeOutput::Kind::Buffer;
    Output.Buffer = std::move(Buffer);
    PendingOutputs[Task] = std::move(Output);
  }

private:
  bool set_path(const char *Dir, SmallString<0> &Slot, bool &Present,
                const char *Label) {
    if (Dir == nullptr) {
      SmallString<96> Message("LLVM ThinLTO ");
      Message += Label;
      Message += " directory was null";
      set_error(Message);
      return false;
    }
    Slot = Dir;
    Present = true;
    return true;
  }

  void clear_outputs() {
    llvm::sys::SmartScopedLock<true> Guard(Mutex);
    PendingOutputs.clear();
    OrderedOutputs.clear();
    LastError.clear();
  }

  void finalize_outputs() {
    llvm::sys::SmartScopedLock<true> Guard(Mutex);
    SmallVector<unsigned, 0> Tasks;
    Tasks.reserve(PendingOutputs.size());
    for (const auto &Entry : PendingOutputs) {
      Tasks.push_back(Entry.first);
    }
    llvm::sort(Tasks);

    OrderedOutputs.clear();
    OrderedOutputs.reserve(Tasks.size());
    for (unsigned Task : Tasks) {
      auto It = PendingOutputs.find(Task);
      if (It != PendingOutputs.end()) {
        OrderedOutputs.push_back(std::move(It->second));
      }
    }
    PendingOutputs.clear();
  }

  void record_file_output(unsigned Task, StringRef Path) {
    llvm::sys::SmartScopedLock<true> Guard(Mutex);
    BridgeOutput Output;
    Output.KindTag = BridgeOutput::Kind::File;
    Output.Path = Path;
    PendingOutputs[Task] = std::move(Output);
  }

  SmallString<0> make_output_path(unsigned Task) const {
    SmallString<0> Path(GeneratedObjectsDir);
#if defined(_WIN32)
    const SmallString<32> FileName(("thinlto-" + Twine(Task) + ".obj").str());
#else
    const SmallString<32> FileName(("thinlto-" + Twine(Task) + ".o").str());
#endif
    llvm::sys::path::append(Path, FileName);
    return Path;
  }

  void set_error(StringRef Message) {
    llvm::sys::SmartScopedLock<true> Guard(Mutex);
    LastError = Message;
  }

  SmallString<32> CpuName = "generic";
  bool HasGeneratedObjectsDir = false;
  bool HasCacheDir = false;
  SmallString<0> GeneratedObjectsDir;
  SmallString<0> CacheDir;
  SmallVector<BridgeModule, 0> Modules;
  mutable llvm::sys::SmartMutex<true> Mutex;
  llvm::DenseMap<unsigned, BridgeOutput> PendingOutputs;
  SmallVector<BridgeOutput, 0> OrderedOutputs;
  SmallString<0> LastError;
};

BufferStream::~BufferStream() {
  flush();
  Session.record_buffer_output(Task, std::move(Buffer));
}

} // namespace

extern "C" {

ThinLtoSession *kern_thinlto_session_create() {
  return new (std::nothrow) ThinLtoSession();
}

void kern_thinlto_session_dispose(ThinLtoSession *Session) { delete Session; }

int kern_thinlto_session_set_cpu(ThinLtoSession *Session, const char *Cpu) {
  return Session != nullptr && Session->set_cpu(Cpu);
}

int kern_thinlto_session_set_generated_objects_dir(ThinLtoSession *Session,
                                                   const char *Dir) {
  return Session != nullptr && Session->set_generated_objects_dir(Dir);
}

int kern_thinlto_session_set_cache_dir(ThinLtoSession *Session, const char *Dir) {
  return Session != nullptr && Session->set_cache_dir(Dir);
}

int kern_thinlto_session_add_module(ThinLtoSession *Session, const char *Identifier,
                                    const std::uint8_t *Bitcode, size_t Size) {
  return Session != nullptr && Session->add_module(Identifier, Bitcode, Size);
}

int kern_thinlto_session_process(ThinLtoSession *Session) {
  return Session != nullptr && Session->process();
}

size_t kern_thinlto_session_object_count(const ThinLtoSession *Session) {
  return Session == nullptr ? 0 : Session->object_count();
}

int kern_thinlto_session_object_is_file(const ThinLtoSession *Session, size_t Index) {
  return Session != nullptr && Session->object_is_file(Index);
}

size_t kern_thinlto_session_object_path_len(const ThinLtoSession *Session, size_t Index) {
  return Session == nullptr ? 0 : Session->object_path_len(Index);
}

int kern_thinlto_session_copy_object_path(const ThinLtoSession *Session, size_t Index,
                                          char *Dest, size_t DestLen) {
  return Session != nullptr && Session->copy_object_path(Index, Dest, DestLen);
}

size_t kern_thinlto_session_object_buffer_len(const ThinLtoSession *Session, size_t Index) {
  return Session == nullptr ? 0 : Session->object_buffer_len(Index);
}

int kern_thinlto_session_copy_object_buffer(const ThinLtoSession *Session, size_t Index,
                                            std::uint8_t *Dest, size_t DestLen) {
  return Session != nullptr && Session->copy_object_buffer(Index, Dest, DestLen);
}

const char *kern_thinlto_session_last_error(const ThinLtoSession *Session) {
  if (Session == nullptr) {
    return "";
  }
  return Session->last_error();
}

} // extern "C"
