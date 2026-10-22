use super::*;
use std::cell::Cell;
use windows::Win32::UI::TextServices::*;
#[implement(ITfDisplayAttributeInfo)]
pub struct Attribute;
impl ITfDisplayAttributeInfo_Impl for Attribute_Impl {
    fn GetGUID(&self) -> Result<GUID> {
        Ok(ATTR_ZHIZI)
    }
    fn GetDescription(&self) -> Result<BSTR> {
        Ok(BSTR::from("知字组字"))
    }
    fn GetAttributeInfo(&self, out: *mut TF_DISPLAYATTRIBUTE) -> Result<()> {
        if out.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            *out = TF_DISPLAYATTRIBUTE {
                lsStyle: TF_LS_SOLID,
                bAttr: TF_ATTR_INPUT,
                ..Default::default()
            };
        }
        Ok(())
    }
    fn SetAttributeInfo(&self, _: *const TF_DISPLAYATTRIBUTE) -> Result<()> {
        Err(E_NOTIMPL.into())
    }
    fn Reset(&self) -> Result<()> {
        Ok(())
    }
}
#[implement(IEnumTfDisplayAttributeInfo)]
pub struct Attributes(pub Cell<bool>);
impl IEnumTfDisplayAttributeInfo_Impl for Attributes_Impl {
    fn Clone(&self) -> Result<IEnumTfDisplayAttributeInfo> {
        Ok(Attributes(Cell::new(self.0.get())).into())
    }
    fn Next(
        &self,
        count: u32,
        out: *mut Option<ITfDisplayAttributeInfo>,
        fetched: *mut u32,
    ) -> Result<()> {
        if out.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe {
            if !fetched.is_null() {
                *fetched = 0;
            }
            if count == 0 {
                return Ok(());
            }
            std::ptr::write(out, None);
            if self.0.replace(true) {
                return Err(Error::from_hresult(S_FALSE));
            }
            std::ptr::write(out, Some(Attribute.into()));
            if !fetched.is_null() {
                *fetched = 1;
            }
        }
        Ok(())
    }
    fn Reset(&self) -> Result<()> {
        self.0.set(false);
        Ok(())
    }
    fn Skip(&self, _: u32) -> Result<()> {
        self.0.set(true);
        Ok(())
    }
}
