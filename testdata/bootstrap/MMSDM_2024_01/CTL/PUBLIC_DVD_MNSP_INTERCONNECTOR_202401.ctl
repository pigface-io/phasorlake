-- ************************************************************
-- Title:	PUBLIC_DVD_MNSP_INTERCONNECTOR_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS MNSP_INTERCONNECTOR owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_MNSP_INTERCONNECTOR_202401010000.CSV
APPEND INTO TABLE MNSP_INTERCONNECTOR
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
LINKID CHAR(10),
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss",
VERSIONNO FLOAT EXTERNAL,
INTERCONNECTORID CHAR(10),
FROMREGION CHAR(10),
TOREGION CHAR(10),
MAXCAPACITY FLOAT EXTERNAL,
TLF FLOAT EXTERNAL,
LHSFACTOR FLOAT EXTERNAL,
METERFLOWCONSTANT FLOAT EXTERNAL,
AUTHORISEDDATE DATE "yyyy/mm/dd hh24:mi:ss",
AUTHORISEDBY CHAR(15),
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
FROM_REGION_TLF FLOAT EXTERNAL,
TO_REGION_TLF FLOAT EXTERNAL)
-- End of Script
