using System.Runtime.InteropServices;
using System.Text;

namespace Glancy.Core.Shell;

/// <summary>A task in the taskbar Jump List: launches the app with <see cref="Arguments"/>.</summary>
public sealed record JumpListTask(string Title, string Arguments, string? Description = null)
{
    /// <summary>A divider line between groups of tasks.</summary>
    public static readonly JumpListTask Separator = new(string.Empty, string.Empty);

    public bool IsSeparator => Title.Length == 0;
}

/// <summary>
/// Writes the "Tasks" section of the app's Jump List (right-click on its taskbar or Start icon) with
/// <c>ICustomDestinationList</c>. Windows stores the list per app, so nothing has to keep running afterwards.
/// No AppUserModelID is set: the list belongs to the default one Windows derives from the exe path, which is also
/// what a pinned shortcut to the exe uses.
/// </summary>
public static class JumpList
{
    private static readonly Guid CLSID_DestinationList = new("77f10cf0-3db5-4966-b520-b7c54fd35ed6");
    private static readonly Guid CLSID_EnumerableObjectCollection = new("2d3468c1-36a7-43b6-ac24-d3f02fd9607a");
    private static readonly Guid CLSID_ShellLink = new("00021401-0000-0000-C000-000000000046");
    private static readonly Guid IID_IObjectArray = new("92CA9DCD-5622-4bba-A805-5E9F541BD8C9");
    private static readonly PropertyKey PKEY_Title = new(new Guid("F29F85E0-4FF9-1068-AB91-08002B27B3D9"), 2);
    private static readonly PropertyKey PKEY_AppUserModel_IsDestListSeparator = new(new Guid("9F4C2855-9F79-4B39-A8D0-E1D42DE1D5F3"), 6);

    /// <summary>Replaces the Jump List tasks. Runs on its own STA thread; throws <see cref="COMException"/> on failure.</summary>
    public static Task SetTasksAsync(string exePath, IReadOnlyList<JumpListTask> tasks) =>
        StaRunner.Run(() =>
        {
            SetTasks(exePath, tasks);
            return true;
        });

    /// <summary>Deletes the Jump List (the uninstaller runs this). Throws <see cref="COMException"/> on failure.</summary>
    public static Task DeleteAsync() =>
        StaRunner.Run(() =>
        {
            var list = Create<ICustomDestinationList>(CLSID_DestinationList);
            try
            {
                list.DeleteList(null);
            }
            finally
            {
                Marshal.FinalReleaseComObject(list);
            }
            return true;
        });

    private static void SetTasks(string exePath, IReadOnlyList<JumpListTask> tasks)
    {
        var list = Create<ICustomDestinationList>(CLSID_DestinationList);
        var collection = Create<IObjectCollection>(CLSID_EnumerableObjectCollection);
        var begun = false;
        try
        {
            var iid = IID_IObjectArray;
            list.BeginList(out _, ref iid, out var removed);
            Marshal.ReleaseComObject(removed);
            begun = true;

            foreach (var task in tasks) collection.AddObject(CreateLink(exePath, task));
            list.AddUserTasks((IObjectArray)collection);
            list.CommitList();
            begun = false;
        }
        finally
        {
            if (begun) list.AbortList();
            Marshal.FinalReleaseComObject(collection);
            Marshal.FinalReleaseComObject(list);
        }
    }

    private static IShellLinkW CreateLink(string exePath, JumpListTask task)
    {
        var link = Create<IShellLinkW>(CLSID_ShellLink);
        var store = (IPropertyStore)link;
        if (task.IsSeparator)
        {
            var separator = PropVariant.FromBool(true);
            var key = PKEY_AppUserModel_IsDestListSeparator;
            store.SetValue(ref key, ref separator);
        }
        else
        {
            link.SetPath(exePath);
            link.SetArguments(task.Arguments);
            link.SetIconLocation(exePath, 0);
            if (task.Description is not null) link.SetDescription(task.Description);
            var title = PropVariant.FromString(task.Title);
            try
            {
                var key = PKEY_Title;
                store.SetValue(ref key, ref title);
            }
            finally
            {
                title.Free();
            }
        }
        store.Commit();
        return link;
    }

    private static T Create<T>(Guid clsid) => (T)Activator.CreateInstance(Type.GetTypeFromCLSID(clsid, throwOnError: true)!)!;

    [StructLayout(LayoutKind.Sequential, Pack = 4)]
    private readonly record struct PropertyKey(Guid FormatId, uint PropertyId);

    /// <summary>The two PROPVARIANT shapes needed here: VT_LPWSTR and VT_BOOL.</summary>
    [StructLayout(LayoutKind.Explicit, Size = 16)]
    private struct PropVariant
    {
        private const ushort VT_BOOL = 11, VT_LPWSTR = 31;
        [FieldOffset(0)] private ushort _type;
        [FieldOffset(8)] private IntPtr _pointer;
        [FieldOffset(8)] private short _bool;

        public static PropVariant FromString(string value) => new() { _type = VT_LPWSTR, _pointer = Marshal.StringToCoTaskMemUni(value) };

        public static PropVariant FromBool(bool value) => new() { _type = VT_BOOL, _bool = (short)(value ? -1 : 0) };

        public void Free()
        {
            if (_type == VT_LPWSTR && _pointer != IntPtr.Zero) Marshal.FreeCoTaskMem(_pointer);
            _pointer = IntPtr.Zero;
        }
    }

    [ComImport, Guid("6332debf-87b5-4670-90c0-5e57b408a49e"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface ICustomDestinationList
    {
        void SetAppID([MarshalAs(UnmanagedType.LPWStr)] string appId);
        void BeginList(out uint minSlots, ref Guid riid, [MarshalAs(UnmanagedType.Interface)] out object removed);
        void AppendCategory([MarshalAs(UnmanagedType.LPWStr)] string category, IObjectArray items);
        void AppendKnownCategory(int category);
        void AddUserTasks(IObjectArray tasks);
        void CommitList();
        void GetRemovedDestinations(ref Guid riid, [MarshalAs(UnmanagedType.Interface)] out object removed);
        void DeleteList([MarshalAs(UnmanagedType.LPWStr)] string? appId);
        void AbortList();
    }

    [ComImport, Guid("92CA9DCD-5622-4bba-A805-5E9F541BD8C9"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IObjectArray
    {
        void GetCount(out uint count);
        void GetAt(uint index, ref Guid riid, [MarshalAs(UnmanagedType.Interface)] out object item);
    }

    [ComImport, Guid("5632b1a4-e38a-400a-928a-d4cd63230295"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IObjectCollection
    {
        // IObjectArray
        void GetCount(out uint count);
        void GetAt(uint index, ref Guid riid, [MarshalAs(UnmanagedType.Interface)] out object item);
        // IObjectCollection
        void AddObject([MarshalAs(UnmanagedType.Interface)] object item);
        void AddFromArray(IObjectArray source);
        void RemoveObjectAt(uint index);
        void Clear();
    }

    [ComImport, Guid("000214F9-0000-0000-C000-000000000046"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IShellLinkW
    {
        void GetPath([MarshalAs(UnmanagedType.LPWStr)] StringBuilder file, int max, IntPtr findData, uint flags);
        void GetIDList(out IntPtr idList);
        void SetIDList(IntPtr idList);
        void GetDescription([MarshalAs(UnmanagedType.LPWStr)] StringBuilder name, int max);
        void SetDescription([MarshalAs(UnmanagedType.LPWStr)] string name);
        void GetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] StringBuilder dir, int max);
        void SetWorkingDirectory([MarshalAs(UnmanagedType.LPWStr)] string dir);
        void GetArguments([MarshalAs(UnmanagedType.LPWStr)] StringBuilder args, int max);
        void SetArguments([MarshalAs(UnmanagedType.LPWStr)] string args);
        void GetHotkey(out short hotkey);
        void SetHotkey(short hotkey);
        void GetShowCmd(out int showCmd);
        void SetShowCmd(int showCmd);
        void GetIconLocation([MarshalAs(UnmanagedType.LPWStr)] StringBuilder path, int max, out int index);
        void SetIconLocation([MarshalAs(UnmanagedType.LPWStr)] string path, int index);
        void SetRelativePath([MarshalAs(UnmanagedType.LPWStr)] string path, uint reserved);
        void Resolve(IntPtr hwnd, uint flags);
        void SetPath([MarshalAs(UnmanagedType.LPWStr)] string file);
    }

    [ComImport, Guid("886d8eeb-8cf2-4446-8d02-cdba1dbdcf99"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IPropertyStore
    {
        void GetCount(out uint count);
        void GetAt(uint index, out PropertyKey key);
        void GetValue(ref PropertyKey key, out PropVariant value);
        void SetValue(ref PropertyKey key, ref PropVariant value);
        void Commit();
    }
}
