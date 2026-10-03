-- ************************************************************
-- Title:	PUBLIC_DVD_REGIONAPC_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS REGIONAPC owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_REGIONAPC_202401010000.CSV
APPEND INTO TABLE REGIONAPC
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
REGIONID CHAR(10),
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss",
VERSIONNO FLOAT EXTERNAL,
AUTHORISEDDATE DATE "yyyy/mm/dd hh24:mi:ss",
AUTHORISEDBY CHAR(10),
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss")
-- End of Script
