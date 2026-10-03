-- ************************************************************
-- Title:	PUBLIC_ARCHIVE#MNSP_INTERCONNECTOR#FILE01#202608.ctl
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
INFILE PUBLIC_ARCHIVE#MNSP_INTERCONNECTOR#FILE01#202608010000.CSV
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
EFFECTIVEDATE DATE "YYYY/MM/DD HH24:MI:SS",
VERSIONNO FLOAT EXTERNAL,
INTERCONNECTORID CHAR(10),
FROMREGION CHAR(10),
TOREGION CHAR(10),
MAXCAPACITY FLOAT EXTERNAL,
TLF FLOAT EXTERNAL,
LHSFACTOR FLOAT EXTERNAL,
METERFLOWCONSTANT FLOAT EXTERNAL,
AUTHORISEDDATE DATE "YYYY/MM/DD HH24:MI:SS",
AUTHORISEDBY CHAR(15),
LASTCHANGED DATE "YYYY/MM/DD HH24:MI:SS",
FROM_REGION_TLF FLOAT EXTERNAL,
TO_REGION_TLF FLOAT EXTERNAL)
-- End of Script
