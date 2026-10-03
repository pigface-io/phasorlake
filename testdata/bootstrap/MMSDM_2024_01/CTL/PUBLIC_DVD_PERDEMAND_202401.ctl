-- ************************************************************
-- Title:	PUBLIC_DVD_PERDEMAND_202401.ctl
-- ************************************************************
-- Project : NEMMCO MONTHLY DVD RELEASE
-- Purpose : Control File to load monthly csv file
-- Target Application  : Monthly DVD Release
-- Application Version : 1
-- Visibility Id : PUBLIC
--
-- ************************************************************
-- Special Instructions : 1. USER TO CONNECT AS PERDEMAND owner
--
-- ************************************************************
-- History :
--      MMS Data Model Version   Template Generated On
--      ======================   ======================
--      PUBLIC			       v
-- ************************************************************
LOAD DATA
INFILE PUBLIC_DVD_PERDEMAND_202401010000.CSV
APPEND INTO TABLE PERDEMAND
WHEN (1:1) = 'D'
FIELDS TERMINATED BY ','
OPTIONALLY ENCLOSED BY '"'
TRAILING NULLCOLS
(row_type FILLER,
report_type FILLER,
report_subtype FILLER,
report_version FILLER,
EFFECTIVEDATE DATE "yyyy/mm/dd hh24:mi:ss",
SETTLEMENTDATE DATE "yyyy/mm/dd hh24:mi:ss",
REGIONID CHAR(10),
OFFERDATE DATE "yyyy/mm/dd hh24:mi:ss",
PERIODID FLOAT EXTERNAL,
VERSIONNO FLOAT EXTERNAL,
RESDEMAND FLOAT EXTERNAL,
DEMAND90PROBABILITY FLOAT EXTERNAL,
DEMAND10PROBABILITY FLOAT EXTERNAL,
LASTCHANGED DATE "yyyy/mm/dd hh24:mi:ss",
MR_SCHEDULE FLOAT EXTERNAL)
-- End of Script
