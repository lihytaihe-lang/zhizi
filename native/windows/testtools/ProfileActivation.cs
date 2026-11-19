using System;
using System.Runtime.InteropServices;

/// <summary>
/// Activates an already-registered TSF input processor profile for the CURRENT PROCESS ONLY.
/// Must be called from an STA WinForms UI thread; makes no registry or machine-global changes.
/// </summary>
public static class ZhiziProfile
{
    private static readonly Guid ClsidInputProcessorProfiles = new Guid("33c53a50-f456-4884-b049-85fd643ecfed");
    private static readonly Guid ClsidProfile = new Guid("371930f8-3da5-4b10-90a3-88f486c9a251");
    private static readonly Guid GuidProfile = new Guid("371930f8-3da5-4b10-90a3-88f486c9a252");

    private const uint TF_PROFILETYPE_INPUTPROCESSOR = 1;
    private const ushort LANGID_ZH_CN = 0x0804;
    private const uint TF_IPP_FLAG_FORPROCESS = 0x10000000;
    private const uint TF_IPP_FLAG_DONTCARECURRENTINPUTLANGUAGE = 0x00000004;

    /// <summary>
    /// ITfInputProcessorProfileMgr binding. Only the first vtable slot (ActivateProfile) is declared
    /// because no other method of the interface is ever called here.
    /// </summary>
    [ComImport]
    [Guid("71c6e74c-0f28-11d8-a82a-00065b84435c")]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface ITfInputProcessorProfileMgr
    {
        /// <summary>Activates the profile for this process only; PreserveSig returns the raw HRESULT.</summary>
        [PreserveSig]
        int ActivateProfile(uint type, ushort langid, ref Guid clsid, ref Guid profile, IntPtr hkl, uint flags);
    }

    /// <summary>
    /// Activates the Zhizi TSF profile for this process only (TF_IPP_FLAG_FORPROCESS; ENABLEPROFILE,
    /// FORSESSION and global flags are deliberately NOT set). Designed to be invoked from the STA
    /// WinForms UI thread, so the profile change affects only this process's input state.
    /// </summary>
    /// <returns>S_OK (0) on success, otherwise the failing HRESULT.</returns>
    public static int Activate()
    {
        object comObject = null;
        try
        {
            Type type = Type.GetTypeFromCLSID(ClsidInputProcessorProfiles);
            comObject = Activator.CreateInstance(type);
            ITfInputProcessorProfileMgr mgr = (ITfInputProcessorProfileMgr)comObject;
            Guid clsid = ClsidProfile;
            Guid profile = GuidProfile;
            return mgr.ActivateProfile(
                TF_PROFILETYPE_INPUTPROCESSOR,
                LANGID_ZH_CN,
                ref clsid,
                ref profile,
                IntPtr.Zero,
                TF_IPP_FLAG_FORPROCESS | TF_IPP_FLAG_DONTCARECURRENTINPUTLANGUAGE);
        }
        catch (Exception ex)
        {
            return Marshal.GetHRForException(ex);
        }
        finally
        {
            if (comObject != null)
            {
                Marshal.ReleaseComObject(comObject);
            }
        }
    }
}
