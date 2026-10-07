-- ************************************************************
-- Title:	PUBLIC_DVD_DUDETAIL_202206.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS DUDETAIL owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_DUDETAIL_202206010000.CSV
APPEND INTO TABLE DUDETAIL
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss",
DUID CHAR(10),
VERSIONNO FLOAT EXTERNAL,
CONNECTIONPOINTID CHAR(10),
VOLTLEVEL CHAR(10),
REGISTEREDCAPACITY FLOAT EXTERNAL,
AGCCAPABILITY CHAR(1),
DISPATCHTYPE CHAR(20),
MAXCAPACITY FLOAT EXTERNAL,
STARTTYPE CHAR(20),
NORMALLYONFLAG CHAR(1),
PHYSICALDETAILSFLAG CHAR(1),
SPINNINGRESERVEFLAG CHAR(1),
AUTHORISEDBY CHAR(15),
AUTHORISEDDATE DATE "yyyy/mm/dd hh24:mi:ss",
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
INTERMITTENTFLAG CHAR(1),
SEMISCHEDULE_FLAG CHAR(1),
MAXRATEOFCHANGEUP FLOAT EXTERNAL,
MAXRATEOFCHANGEDOWN FLOAT EXTERNAL,
DISPATCHSUBTYPE CHAR(20))
-- End of Script
