-- ************************************************************
-- Title:	PUBLIC_DVD_BIDTYPES_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS BIDTYPES owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_BIDTYPES_202401010000.CSV
APPEND INTO TABLE BIDTYPES
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
BIDTYPE CHAR(10),
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss",
VERSIONNO FLOAT EXTERNAL,
DESCRIPTION CHAR(64),
NUMBEROFBANDS FLOAT EXTERNAL,
NUMDAYSAHEADPRICELOCKED FLOAT EXTERNAL,
VALIDATIONRULE CHAR(10),
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
SPDALIAS CHAR(10))
-- End of Script
